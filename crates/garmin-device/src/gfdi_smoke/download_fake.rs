use std::collections::VecDeque;
use std::io::Write as _;
use std::num::NonZeroU8;
use std::time::Duration;

use async_trait::async_trait;
use buffa::Message as _;
use flate2::{Compression, write::ZlibEncoder};
use garmin_gfdi::proto::{Smart, data_types, file_access as wire};
use garmin_gfdi::rpc::encode_rpc_request;
use garmin_gfdi::{
    Characteristic, FrameDecoder, LinkMode, Message, MlrReceiver, MlrSender, Route, encode_frame,
    encode_message,
};
use tokio::time::Instant;

use super::{SmokeError, Transport, fake::SyntheticPeer};

struct PeerChannel {
    handle: NonZeroU8,
    mode: LinkMode,
    receiver: MlrReceiver,
    sender: MlrSender,
}

impl PeerChannel {
    fn new(handle: u8, mode: LinkMode) -> Result<Self, SmokeError> {
        let handle = NonZeroU8::new(handle).ok_or(SmokeError::Preconditions("zero fake handle"))?;
        Ok(Self {
            handle,
            mode,
            receiver: MlrReceiver::new(handle, 512).map_err(|_| invalid())?,
            sender: MlrSender::new(handle, 20, 64 * 1024, 5).map_err(|_| invalid())?,
        })
    }

    fn emit(&mut self, bytes: &[u8], pending: &mut VecDeque<Vec<u8>>) -> Result<(), SmokeError> {
        if self.mode == LinkMode::Reliable {
            self.sender.enqueue(bytes).map_err(|_| invalid())?;
        } else {
            for chunk in bytes.chunks(19) {
                let mut packet = vec![self.handle.get()];
                packet.extend(chunk);
                pending.push_back(packet);
            }
        }
        Ok(())
    }

    fn receive(
        &mut self,
        bytes: &[u8],
        now: Duration,
        pending: &mut VecDeque<Vec<u8>>,
    ) -> Result<Option<Vec<u8>>, SmokeError> {
        if self.mode == LinkMode::Plain {
            return Ok((bytes[0] == self.handle.get()).then(|| bytes[1..].to_vec()));
        }
        if bytes[0] & 0x80 == 0 || bytes[0] >> 4 & 7 != self.handle.get() & 7 {
            return Ok(None);
        }
        let update = self.receiver.receive(bytes, now).map_err(|_| invalid())?;
        self.sender
            .acknowledge(update.peer_ack, now)
            .map_err(|_| invalid())?;
        pending.extend(update.ack.map(Vec::from));
        let data = update.data.map(<[u8]>::to_vec);
        pending.extend(
            self.receiver
                .flush_ack()
                .map_err(|_| invalid())?
                .map(Vec::from),
        );
        Ok(data)
    }

    fn pump(&mut self, now: Duration, pending: &mut VecDeque<Vec<u8>>) -> Result<(), SmokeError> {
        if self.mode == LinkMode::Reliable {
            pending.extend(
                self.sender
                    .poll(now, self.receiver.expected_sequence())
                    .map_err(|_| invalid())?,
            );
        }
        Ok(())
    }
}

fn invalid() -> SmokeError {
    SmokeError::Preconditions("synthetic FileAccess peer rejected exchange")
}

pub(super) struct FilePeer {
    inventory: SyntheticPeer,
    pending: VecDeque<Vec<u8>>,
    decoder: FrameDecoder,
    main: Option<PeerChannel>,
    pipe: Option<PeerChannel>,
    client: [u8; 8],
    start: Instant,
    compressed: Vec<u8>,
    source: Vec<u8>,
    pipe_sent: bool,
    pipe_closed: bool,
    pub(super) disconnected: bool,
    pub(super) writes: Vec<Vec<u8>>,
}

impl FilePeer {
    pub(super) fn new() -> Result<Self, SmokeError> {
        let source = garmin_fit::fixture::activity(garmin_fit::fixture::Sport::Cycling)
            .map_err(|_| invalid())?;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&source)?;
        let compressed = encoder.finish()?;
        Ok(Self {
            inventory: SyntheticPeer::new(),
            pending: VecDeque::new(),
            decoder: FrameDecoder::new(10000),
            main: None,
            pipe: None,
            client: [0; 8],
            start: Instant::now(),
            compressed,
            source,
            pipe_sent: false,
            pipe_closed: false,
            disconnected: false,
            writes: Vec::new(),
        })
    }

    #[cfg(test)]
    pub(super) fn source(&self) -> &[u8] {
        &self.source
    }

    fn identity(&mut self) -> Result<(), SmokeError> {
        let mut payload = Vec::new();
        payload.extend(150_u16.to_le_bytes());
        payload.extend(0x1234_u16.to_le_bytes());
        payload.extend(0x0102_0304_u32.to_le_bytes());
        payload.extend(1234_u16.to_le_bytes());
        payload.extend(375_u16.to_le_bytes());
        for name in ["Demo Garmin", "Synthetic peer", "Demo tracker"] {
            payload.push(u8::try_from(name.len()).map_err(|_| invalid())?);
            payload.extend(name.as_bytes());
        }
        self.emit_main(&encode_frame(
            &encode_message(5024, &payload).map_err(|_| invalid())?,
        ))
    }

    fn emit_main(&mut self, bytes: &[u8]) -> Result<(), SmokeError> {
        self.main
            .as_mut()
            .ok_or_else(invalid)?
            .emit(bytes, &mut self.pending)
    }

    fn reply(&mut self, service: wire::Service, id: u16) -> Result<(), SmokeError> {
        let bytes = Smart {
            file_access_service: service.into(),
            ..Default::default()
        }
        .encode_to_vec();
        self.emit_main(&encode_frame(
            &encode_rpc_request(id, &bytes, 1000).map_err(|_| invalid())?,
        ))
    }

    fn message(&mut self, bytes: &[u8]) -> Result<(), SmokeError> {
        let message = Message::decode(bytes).map_err(|_| invalid())?;
        if message.message_id() == 5000 {
            return Ok(());
        }
        if message.message_id() != 5043 || message.payload().len() < 14 {
            return Err(invalid());
        }
        let mut smart =
            Smart::decode_from_slice(&message.payload()[14..]).map_err(|_| invalid())?;
        let mut service = smart.file_access_service.take().ok_or_else(invalid)?;
        if service.item_list_request.take().is_some() {
            if service.encoded_len() != 0 {
                return Err(invalid());
            }
            let item = wire::ItemReference {
                uid: data_types::UUID {
                    most_significant: 1,
                    least_significant: 2,
                    ..Default::default()
                }
                .into(),
                data_type: wire::DataType {
                    gdxml_string: Some("FIT_TYPE_4".into()),
                    ..Default::default()
                }
                .into(),
                data_size: Some(self.source.len() as u64),
                ..Default::default()
            };
            self.reply(
                wire::Service {
                    item_list_response: wire::ItemListResponse {
                        status: Some(wire::item_list_response::Status::SUCCESS),
                        items: vec![item],
                        ..Default::default()
                    }
                    .into(),
                    ..Default::default()
                },
                77,
            )?;
        } else if let Some(pull) = service.pull_item_request.take() {
            if service.encoded_len() != 0
                || pull.item.uid.most_significant != 1
                || pull.item.uid.least_significant != 2
            {
                return Err(invalid());
            }
            self.reply(
                wire::Service {
                    pull_item_response: wire::PullItemDataResponse {
                        result: Some(wire::ItemAccessResult::SUCCESS),
                        transport: Some(wire::TransportProtocol::MULTILINK_TRANSPORT_PIPE),
                        transfer_handle: Some(7),
                        compression_window: Some(15),
                        ..Default::default()
                    }
                    .into(),
                    ..Default::default()
                },
                78,
            )?;
        } else {
            return Err(invalid());
        }
        Ok(())
    }

    fn pump(&mut self) -> Result<(), SmokeError> {
        let now = self.start.elapsed();
        if let Some(main) = &mut self.main {
            main.pump(now, &mut self.pending)?;
        }
        if let Some(pipe) = &mut self.pipe {
            pipe.pump(now, &mut self.pending)?;
            if self.pipe_sent && !self.pipe_closed && pipe.sender.buffered_bytes() == 0 {
                let mut close = vec![0, 3];
                close.extend(self.client);
                close.extend(0x2018_u16.to_le_bytes());
                close.extend([0x82, 0]);
                self.pending.push_back(close);
                self.pipe_closed = true;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Transport for FilePeer {
    async fn connect(&mut self) -> Result<(), SmokeError> {
        Ok(())
    }
    async fn inventory(&mut self) -> Result<Vec<Characteristic>, SmokeError> {
        self.inventory.inventory().await
    }
    async fn subscribe(&mut self, _route: Route) -> Result<(), SmokeError> {
        Ok(())
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<(), SmokeError> {
        self.writes.push(bytes.to_vec());
        if bytes.len() == 13 && bytes[..2] == [0, 0] {
            self.client.copy_from_slice(&bytes[2..10]);
            let service = u16::from_le_bytes([bytes[10], bytes[11]]);
            let mode = if bytes[12] == 2 {
                LinkMode::Reliable
            } else {
                LinkMode::Plain
            };
            let handle = match service {
                1 => 0x86,
                0x2018 => 0x82,
                _ => return Err(invalid()),
            };
            let mut reply = vec![0, 1];
            reply.extend(&bytes[2..12]);
            reply.extend([0, handle, u8::from(mode == LinkMode::Reliable)]);
            self.pending.push_back(reply);
            if service == 1 {
                self.main = Some(PeerChannel::new(handle, mode)?);
                self.identity()?;
            } else {
                self.pipe = Some(PeerChannel::new(handle, mode)?);
            }
        } else if bytes.len() == 13 && bytes[..2] == [0, 2] {
            let mut reply = vec![0, 3];
            reply.extend(&bytes[2..]);
            reply.push(0);
            self.pending.push_back(reply);
        } else {
            if bytes.is_empty() {
                return Err(invalid());
            }
            let now = self.start.elapsed();
            if let Some(main) = &mut self.main
                && let Some(data) = main.receive(bytes, now, &mut self.pending)?
            {
                for byte in data {
                    if let Some(frame) = self.decoder.push(byte) {
                        self.message(&frame.map_err(|_| invalid())?)?;
                    }
                }
            }
            if let Some(pipe) = &mut self.pipe
                && let Some(data) = pipe.receive(bytes, now, &mut self.pending)?
            {
                if data != [0, 0, 7, 0, 0, 0] || self.pipe_sent {
                    return Err(invalid());
                }
                pipe.emit(&[0, 0, 0], &mut self.pending)?;
                pipe.emit(&self.compressed, &mut self.pending)?;
                self.pipe_sent = true;
            }
        }
        self.pump()
    }

    async fn next(&mut self) -> Result<Option<Vec<u8>>, SmokeError> {
        while self.pending.is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
            self.pump()?;
        }
        Ok(self.pending.pop_front())
    }

    async fn disconnect(&mut self) -> Result<(), SmokeError> {
        self.disconnected = true;
        Ok(())
    }
}
