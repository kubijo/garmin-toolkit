use std::collections::VecDeque;

use async_trait::async_trait;
use garmin_gfdi::{Characteristic, Route, encode_frame, encode_message};

use super::{SmokeError, Transport};

pub(super) struct SyntheticPeer {
    pending: VecDeque<Vec<u8>>,
    pub(super) writes: Vec<Vec<u8>>,
    pub(super) disconnected: bool,
}

impl SyntheticPeer {
    pub(super) fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            writes: Vec::new(),
            disconnected: false,
        }
    }
}

#[async_trait]
impl Transport for SyntheticPeer {
    async fn connect(&mut self) -> Result<(), SmokeError> {
        Ok(())
    }

    async fn inventory(&mut self) -> Result<Vec<Characteristic>, SmokeError> {
        let service = 0x6a4e_2800_667b_11e3_949a_0800_200c_9a66;
        Ok(vec![
            Characteristic {
                service,
                uuid: 0x6a4e_2810_667b_11e3_949a_0800_200c_9a66,
                notify: true,
                write: false,
            },
            Characteristic {
                service,
                uuid: 0x6a4e_2820_667b_11e3_949a_0800_200c_9a66,
                notify: false,
                write: true,
            },
        ])
    }

    async fn subscribe(&mut self, _route: Route) -> Result<(), SmokeError> {
        Ok(())
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<(), SmokeError> {
        self.writes.push(bytes.to_vec());
        if bytes.len() == 13 && bytes[..2] == [0, 0] && bytes[10..12] == [1, 0] {
            let mut reply = vec![0, 1];
            reply.extend(&bytes[2..12]);
            reply.extend([0, 0x86, u8::from(bytes[12] == 2)]);
            self.pending.push_back(reply);
            let mut payload = Vec::new();
            payload.extend(150_u16.to_le_bytes());
            payload.extend(0x1234_u16.to_le_bytes());
            payload.extend(0x0102_0304_u32.to_le_bytes());
            payload.extend(1234_u16.to_le_bytes());
            payload.extend(375_u16.to_le_bytes());
            for text in ["Demo Garmin", "Synthetic peer", "Demo tracker"] {
                payload.push(
                    u8::try_from(text.len())
                        .map_err(|_| SmokeError::Preconditions("synthetic string too long"))?,
                );
                payload.extend(text.as_bytes());
            }
            let frame = encode_frame(
                &encode_message(5024, &payload).map_err(garmin_gfdi::ProbeError::from)?,
            );
            for (index, chunk) in frame.chunks(18).enumerate() {
                let mut notification = if bytes[12] == 2 {
                    vec![
                        0xe0,
                        u8::try_from(index)
                            .map_err(|_| SmokeError::Preconditions("synthetic frame too long"))?,
                    ]
                } else {
                    vec![0x86]
                };
                notification.extend(chunk);
                self.pending.push_back(notification);
            }
        } else if bytes.len() == 13 && bytes[..2] == [0, 2] {
            let mut reply = vec![0, 3];
            reply.extend(&bytes[2..]);
            reply.push(0);
            self.pending.push_back(reply);
        }
        Ok(())
    }

    async fn next(&mut self) -> Result<Option<Vec<u8>>, SmokeError> {
        Ok(self.pending.pop_front())
    }

    async fn disconnect(&mut self) -> Result<(), SmokeError> {
        self.disconnected = true;
        Ok(())
    }
}
