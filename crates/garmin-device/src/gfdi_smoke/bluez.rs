use std::pin::Pin;

use async_trait::async_trait;
use bluer::gatt::WriteOp;
use bluer::gatt::remote::{Characteristic as RemoteCharacteristic, CharacteristicWriteRequest};
use futures_util::{Stream, StreamExt};
use garmin_gfdi::{Characteristic, Route};

use super::{SmokeError, Transport};

pub(super) struct BluezTransport {
    _session: bluer::Session,
    device: bluer::Device,
    characteristics: Vec<(Characteristic, RemoteCharacteristic)>,
    incoming: Option<Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>>,
    outgoing: Option<(RemoteCharacteristic, WriteOp)>,
    paired_here: bool,
}

impl BluezTransport {
    pub(super) async fn open(
        adapter_name: &str,
        address: bluer::Address,
    ) -> Result<Self, SmokeError> {
        Self::open_checked(adapter_name, address, false).await
    }

    pub(super) async fn open_for_pairing(
        adapter_name: &str,
        address: bluer::Address,
    ) -> Result<Self, SmokeError> {
        Self::open_checked(adapter_name, address, true).await
    }

    async fn open_checked(
        adapter_name: &str,
        address: bluer::Address,
        pairing: bool,
    ) -> Result<Self, SmokeError> {
        let session = bluer::Session::new().await?;
        let adapter = session.adapter(adapter_name)?;
        if !adapter.is_powered().await? {
            return Err(SmokeError::Preconditions("selected adapter is powered off"));
        }
        let device = adapter.device(address)?;
        if !pairing && !device.is_paired().await? {
            return Err(SmokeError::Preconditions(
                "requires a pre-existing host bond; this command never pairs",
            ));
        }
        if device.is_connected().await? {
            return Err(SmokeError::Preconditions(
                "device is already connected on this host",
            ));
        }
        Ok(Self {
            _session: session,
            device,
            characteristics: Vec::new(),
            incoming: None,
            outgoing: None,
            paired_here: false,
        })
    }
}

#[async_trait]
impl Transport for BluezTransport {
    async fn bond(&mut self) -> Result<(), SmokeError> {
        if !self.device.is_paired().await? {
            eprintln!(
                "Confirm the matching Bluetooth code on the Garmin and in Ubuntu's pairing dialog."
            );
            self.device.pair().await?;
            self.paired_here = true;
        }
        if !self.device.is_paired().await? {
            return Err(SmokeError::Preconditions(
                "BlueZ did not retain a device bond",
            ));
        }
        Ok(())
    }

    async fn connect(&mut self) -> Result<(), SmokeError> {
        if self.paired_here && self.device.is_connected().await? {
            return Ok(());
        }
        self.device.connect().await?;
        Ok(())
    }

    async fn inventory(&mut self) -> Result<Vec<Characteristic>, SmokeError> {
        let services = self.device.services().await?;
        if services.len() > 128 {
            return Err(SmokeError::Preconditions(
                "service inventory limit exceeded",
            ));
        }
        for service in services {
            let service_id = service.uuid().await?.as_u128();
            for characteristic in service.characteristics().await? {
                if self.characteristics.len() >= 1024 {
                    return Err(SmokeError::Preconditions(
                        "characteristic inventory limit exceeded",
                    ));
                }
                let flags = characteristic.flags().await?;
                let metadata = Characteristic {
                    service: service_id,
                    uuid: characteristic.uuid().await?.as_u128(),
                    notify: flags.notify,
                    write: flags.write || flags.write_without_response,
                };
                self.characteristics.push((metadata, characteristic));
            }
        }
        Ok(self
            .characteristics
            .iter()
            .map(|(metadata, _)| *metadata)
            .collect())
    }

    async fn subscribe(&mut self, route: Route) -> Result<(), SmokeError> {
        let receive = self
            .characteristics
            .iter()
            .find(|(meta, _)| meta.service == route.service && meta.uuid == route.receive)
            .map(|(_, characteristic)| characteristic.clone())
            .ok_or(SmokeError::NoRoute)?;
        let send = self
            .characteristics
            .iter()
            .find(|(meta, _)| meta.service == route.service && meta.uuid == route.send)
            .map(|(_, characteristic)| characteristic.clone())
            .ok_or(SmokeError::NoRoute)?;
        let flags = send.flags().await?;
        self.outgoing = Some((
            send,
            if flags.write {
                WriteOp::Request
            } else {
                WriteOp::Command
            },
        ));
        self.incoming = Some(receive.notify().await?.boxed());
        Ok(())
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<(), SmokeError> {
        let (characteristic, op_type) = self
            .outgoing
            .as_ref()
            .ok_or(SmokeError::Preconditions("send characteristic missing"))?;
        let request = CharacteristicWriteRequest {
            op_type: *op_type,
            ..Default::default()
        };
        characteristic.write_ext(bytes, &request).await?;
        Ok(())
    }

    async fn next(&mut self) -> Result<Option<Vec<u8>>, SmokeError> {
        let incoming = self.incoming.as_mut().ok_or(SmokeError::Preconditions(
            "notification subscription missing",
        ))?;
        Ok(incoming.next().await)
    }

    async fn disconnect(&mut self) -> Result<(), SmokeError> {
        self.incoming = None;
        self.outgoing = None;
        self.device.disconnect().await?;
        if self.device.is_connected().await? {
            return Err(SmokeError::Preconditions(
                "device still connected after disconnect",
            ));
        }
        Ok(())
    }
}
