//! One bounded `FileAccess` activity download over caller-owned `MultiLink` transport.

use std::collections::{HashMap, HashSet};
use std::num::{NonZeroU32, NonZeroU64};
use std::time::Duration;

use flate2::{Decompress, FlushDecompress, Status};
use thiserror::Error;

use crate::file_access::{
    FileAccessError, FileAccessResponse, decode_response, list_request, pull_request,
    transfer_status_response,
};
use crate::proto::file_access as wire;
use crate::rpc::{RpcError, RpcReassembly, encode_rpc_request, encode_rpc_response};
use crate::{
    Channel, DeviceInformation, FrameDecoder, FrameError, IdentityError, LinkMode, Message,
    MessageError, MlrError, MlrReceiver, MlrSendError, MlrSender, Registration, RegistrationError,
    RegistrationState, encode_frame,
};

const PIPE_SERVICE: u16 = 0x2018;

/// Rejected GFDI session or transport state.
#[derive(Debug, Error)]
pub enum DownloadError {
    /// Channel registration failed.
    #[error(transparent)]
    Registration(#[from] RegistrationError),
    /// Reliable receive framing failed.
    #[error(transparent)]
    Receive(#[from] MlrError),
    /// Reliable send window failed.
    #[error(transparent)]
    Send(#[from] MlrSendError),
    /// COBS framing failed.
    #[error(transparent)]
    Frame(#[from] FrameError),
    /// GFDI envelope failed validation.
    #[error(transparent)]
    Message(#[from] MessageError),
    /// Device information failed validation.
    #[error(transparent)]
    Identity(#[from] IdentityError),
    /// Protobuf reassembly failed.
    #[error(transparent)]
    Rpc(#[from] RpcError),
    /// `FileAccess` message failed validation.
    #[error(transparent)]
    FileAccess(#[from] FileAccessError),
    /// Compressed data is invalid.
    #[error(transparent)]
    Compression(#[from] flate2::DecompressError),
    /// The bounded exchange violated its explicit contract.
    #[error("GFDI session stopped: {0}")]
    Protocol(&'static str),
}

/// A retained uncompressed file and its original typed item reference.
#[derive(Debug)]
pub struct DownloadedFile {
    /// Exact selected peer reference; never used as a host filesystem path.
    pub item: wire::ItemReference,
    /// Uncompressed bytes; the adapter must validate FIT before publishing them.
    pub bytes: Vec<u8>,
}

pub(super) struct ChannelInput {
    pub(super) opened: bool,
    pub(super) closed: bool,
    pub(super) data: Option<Vec<u8>>,
}

#[derive(Debug)]
pub(super) struct DataChannel {
    registration: Registration,
    channel: Option<Channel>,
    receiver: Option<MlrReceiver>,
    sender: Option<MlrSender>,
    closed: bool,
}

impl DataChannel {
    pub(super) fn begin(
        client: NonZeroU64,
        service: u16,
        mode: LinkMode,
        deadline: Duration,
    ) -> (Self, [u8; 13]) {
        let (registration, request) = Registration::begin_service(client, service, mode, deadline);
        (
            Self {
                registration,
                channel: None,
                receiver: None,
                sender: None,
                closed: false,
            },
            request,
        )
    }

    pub(super) fn receive(
        &mut self,
        packet: &[u8],
        now: Duration,
        outgoing: &mut Vec<Vec<u8>>,
    ) -> Result<ChannelInput, DownloadError> {
        let mut input = ChannelInput {
            opened: false,
            closed: false,
            data: None,
        };
        if self.closed {
            return Ok(input);
        }
        if self.registration.state() == RegistrationState::Pending {
            if let Some(channel) = self.registration.receive(packet, now)? {
                if channel.mode == LinkMode::Reliable {
                    self.receiver = Some(MlrReceiver::new(channel.handle, 512)?);
                    self.sender = Some(MlrSender::new(channel.handle, 20, 64 * 1024, 5)?);
                }
                self.channel = Some(channel);
                input.opened = true;
            }
            return Ok(input);
        }
        let channel = self
            .channel
            .ok_or(DownloadError::Protocol("channel missing"))?;
        if packet[0] == 0 {
            if let Some(close) = self.registration.close_request()
                && packet.len() == 14
                && packet[..2] == [0, 3]
                && packet[2..13] == close[2..13]
            {
                if packet[13] != 0 {
                    return Err(DownloadError::Protocol("peer channel close failed"));
                }
                if let Some(receiver) = &mut self.receiver {
                    outgoing.extend(receiver.flush_ack()?.map(Vec::from));
                }
                self.closed = true;
                input.closed = true;
            }
            return Ok(input);
        }
        if let Some(receiver) = &mut self.receiver {
            if packet[0] & 0x80 == 0 || (packet[0] >> 4) & 7 != channel.handle.get() & 7 {
                return Ok(input);
            }
            let update = receiver.receive(packet, now)?;
            self.sender
                .as_mut()
                .ok_or(DownloadError::Protocol("MLR sender missing"))?
                .acknowledge(update.peer_ack, now)?;
            outgoing.extend(update.ack.map(Vec::from));
            input.data = update.data.map(<[u8]>::to_vec);
        } else if packet[0] == channel.handle.get() {
            input.data = Some(packet[1..].to_vec());
        }
        Ok(input)
    }

    pub(super) fn write(
        &mut self,
        bytes: &[u8],
        outgoing: &mut Vec<Vec<u8>>,
    ) -> Result<(), DownloadError> {
        if self.closed {
            return Err(DownloadError::Protocol("write on closed channel"));
        }
        let channel = self
            .channel
            .ok_or(DownloadError::Protocol("write before channel registration"))?;
        if let Some(sender) = &mut self.sender {
            sender.enqueue(bytes)?;
        } else {
            for chunk in bytes.chunks(19) {
                let mut packet = vec![channel.handle.get()];
                packet.extend(chunk);
                outgoing.push(packet);
            }
        }
        Ok(())
    }

    pub(super) fn poll(
        &mut self,
        now: Duration,
        outgoing: &mut Vec<Vec<u8>>,
    ) -> Result<(), DownloadError> {
        if self.closed {
            return Ok(());
        }
        if self.registration.state() == RegistrationState::Pending {
            self.registration.poll(now)?;
        }
        if let Some(receiver) = &mut self.receiver {
            outgoing.extend(receiver.poll(now)?.map(Vec::from));
        }
        if let Some(sender) = &mut self.sender {
            outgoing.extend(
                sender.poll(
                    now,
                    self.receiver
                        .as_ref()
                        .ok_or(DownloadError::Protocol("MLR receiver missing"))?
                        .expected_sequence(),
                )?,
            );
        }
        Ok(())
    }

    pub(super) fn close_request(&self) -> Option<[u8; 13]> {
        (!self.closed)
            .then(|| self.registration.close_request())
            .flatten()
    }

    pub(super) fn drained(&self) -> bool {
        self.sender
            .as_ref()
            .is_none_or(|sender| sender.buffered_bytes() == 0)
    }
}

/// Bounded one-activity `FileAccess` session, independent of Bluetooth and host storage.
///
/// Emits host-information acknowledgement, unfiltered listing, one pull, protocol ACKs,
/// and one owned transfer-pipe registration. Never sends time/configuration/authentication,
/// legacy file-index commands, archive/delete, or synced-flag mutations. Caller owns every write.
#[derive(Debug)]
pub struct DownloadSession {
    client: NonZeroU64,
    mode: LinkMode,
    deadline: Duration,
    max_bytes: usize,
    main: DataChannel,
    pipe: Option<DataChannel>,
    decoder: FrameDecoder,
    rpc: Option<RpcReassembly>,
    request_id: u16,
    identity: Option<DeviceInformation>,
    selected: Option<wire::ItemReference>,
    pull_started: bool,
    pages: usize,
    seen: HashSet<(u64, u64)>,
    type_names: HashMap<u32, String>,
    transfer: Option<u16>,
    compression: Option<u32>,
    pipe_started: bool,
    compressed: Vec<u8>,
    completed: Option<DownloadedFile>,
    stopped: bool,
}

impl DownloadSession {
    /// Begin service 1 registration with an explicit client ID, mode, deadline and file bound.
    #[must_use]
    pub fn begin(
        client: NonZeroU64,
        mode: LinkMode,
        deadline: Duration,
        max_file_bytes: usize,
    ) -> (Self, [u8; 13]) {
        let (main, request) = DataChannel::begin(client, 1, mode, deadline);
        (
            Self {
                client,
                mode,
                deadline,
                max_bytes: max_file_bytes,
                main,
                pipe: None,
                decoder: FrameDecoder::new(10_000),
                rpc: None,
                request_id: 0,
                identity: None,
                selected: None,
                pull_started: false,
                pages: 0,
                seen: HashSet::new(),
                type_names: HashMap::new(),
                transfer: None,
                compression: None,
                pipe_started: false,
                compressed: Vec::new(),
                completed: None,
                stopped: false,
            },
            request,
        )
    }

    /// Process one ATT notification and return bounded, ordered writes for the caller.
    ///
    /// # Errors
    /// Rejects malformed, expired, unauthenticated, unsupported or over-limit exchanges.
    /// Any error stops this instance; known-handle cleanup remains available.
    pub fn receive(&mut self, packet: &[u8], now: Duration) -> Result<Vec<Vec<u8>>, DownloadError> {
        let result = self.receive_inner(packet, now);
        if result.is_err() {
            self.cancel();
        }
        result
    }

    fn receive_inner(
        &mut self,
        packet: &[u8],
        now: Duration,
    ) -> Result<Vec<Vec<u8>>, DownloadError> {
        self.check(now)?;
        if packet.is_empty() || packet.len() > 512 {
            return Err(DownloadError::Protocol("invalid ATT notification size"));
        }
        let mut outgoing = Vec::new();
        let main = self.main.receive(packet, now, &mut outgoing)?;
        if main.closed {
            return Err(DownloadError::Protocol(
                "GFDI channel closed before completion",
            ));
        }
        if let Some(data) = main.data {
            for byte in data {
                if let Some(frame) = self.decoder.push(byte) {
                    let frame = frame?;
                    self.message(Message::decode(&frame)?, now, &mut outgoing)?;
                }
            }
        }
        if let Some(pipe) = &mut self.pipe {
            let input = pipe.receive(packet, now, &mut outgoing)?;
            if input.opened {
                if pipe
                    .channel
                    .zip(self.main.channel)
                    .is_some_and(|(pipe, main)| match self.mode {
                        LinkMode::Reliable => pipe.handle.get() & 7 == main.handle.get() & 7,
                        LinkMode::Plain => pipe.handle == main.handle,
                    })
                {
                    return Err(DownloadError::Protocol("transfer and GFDI handles collide"));
                }
                let token = self
                    .transfer
                    .ok_or(DownloadError::Protocol("transfer token missing"))?
                    .to_le_bytes();
                pipe.write(&[0, 0, token[0], token[1], 0, 0], &mut outgoing)?;
            }
            if let Some(data) = input.data {
                if self.pipe_started {
                    let limit = self.max_bytes.saturating_mul(2).saturating_add(65536);
                    if data.len() > limit.saturating_sub(self.compressed.len()) {
                        return Err(DownloadError::Protocol("compressed payload limit exceeded"));
                    }
                    self.compressed.extend(data);
                } else {
                    if data != [0, 0, 0] {
                        return Err(DownloadError::Protocol("unexpected transfer-pipe start"));
                    }
                    self.pipe_started = true;
                }
            }
            if input.closed {
                self.finish()?;
            }
        }
        self.poll_channels(now, &mut outgoing)?;
        Ok(outgoing)
    }

    fn message(
        &mut self,
        message: Message<'_>,
        now: Duration,
        outgoing: &mut Vec<Vec<u8>>,
    ) -> Result<(), DownloadError> {
        match message.message_id() {
            5024 if self.identity.is_none() => {
                let identity = DeviceInformation::decode(message.payload())?;
                if identity.max_packet_size < 64 {
                    return Err(DownloadError::Protocol("GFDI packet size too small"));
                }
                self.main.write(
                    &crate::identity::host_information(message, &identity)?,
                    outgoing,
                )?;
                self.identity = Some(identity);
                self.send_rpc(
                    &list_request(
                        None,
                        NonZeroU32::new(100).ok_or(DownloadError::Protocol("invalid page size"))?,
                    ),
                    outgoing,
                )?;
            }
            5043 | 5044 if self.identity.is_some() => {
                let assembly = self
                    .rpc
                    .get_or_insert_with(|| RpcReassembly::new(64 * 1024, self.deadline));
                let update = assembly.receive(message, now)?;
                self.main
                    .write(&encode_frame(&update.acknowledgement), outgoing)?;
                if let Some(bytes) = update.complete {
                    self.rpc = None;
                    self.response(decode_response(&bytes)?, update.request_id, outgoing)?;
                }
            }
            5101..=5112 => {
                return Err(DownloadError::Protocol(
                    "authentication requested; download never pairs or authenticates",
                ));
            }
            _ => {}
        }
        Ok(())
    }

    fn response(
        &mut self,
        response: FileAccessResponse,
        peer_request_id: u16,
        outgoing: &mut Vec<Vec<u8>>,
    ) -> Result<(), DownloadError> {
        match response {
            FileAccessResponse::List(page) if !self.pull_started && self.completed.is_none() => {
                self.list_page(page, outgoing)
            }
            FileAccessResponse::Pull(result)
                if self.pull_started && self.selected.is_some() && self.pipe.is_none() =>
            {
                self.pull_response(&result, outgoing)
            }
            FileAccessResponse::TransferStatus(status) => {
                self.transfer_status(&status, peer_request_id, outgoing)
            }
            _ => Err(DownloadError::Protocol(
                "unexpected FileAccess response phase",
            )),
        }
    }

    fn list_page(
        &mut self,
        page: wire::ItemListResponse,
        outgoing: &mut Vec<Vec<u8>>,
    ) -> Result<(), DownloadError> {
        if page.status != Some(wire::item_list_response::Status::SUCCESS) {
            return Err(DownloadError::Protocol("listing refused or status absent"));
        }
        self.pages += 1;
        if self.pages > 32 || page.items.len() > 1000 {
            return Err(DownloadError::Protocol("listing limit exceeded"));
        }
        let mut progress = false;
        for item in page.items {
            let Some(uid) = item.uid.as_option() else {
                return Err(DownloadError::Protocol("listed item UID absent"));
            };
            let Some(kind) = item.data_type.as_option() else {
                return Err(DownloadError::Protocol("listed item type absent"));
            };
            if let (Some(key), Some(name)) = (kind.gdxml_string_key, &kind.gdxml_string) {
                if self
                    .type_names
                    .get(&key)
                    .is_some_and(|previous| previous != name)
                {
                    return Err(DownloadError::Protocol("type-name key changed"));
                }
                self.type_names.insert(key, name.clone());
            }
            if self.seen.len() >= 10000 {
                return Err(DownloadError::Protocol("item limit exceeded"));
            }
            progress |= self
                .seen
                .insert((uid.most_significant, uid.least_significant));
            let name = kind.gdxml_string.as_ref().or_else(|| {
                kind.gdxml_string_key
                    .and_then(|key| self.type_names.get(&key))
            });
            if self.selected.is_none() && name.is_some_and(|name| name == "FIT_TYPE_4") {
                pull_request(&item, self.max_bytes as u64)?;
                self.selected = Some(item);
            }
        }
        if !progress && page.session_id.is_some() {
            return Err(DownloadError::Protocol("listing made no progress"));
        }
        if let Some(cursor) = page.session_id {
            self.send_rpc(
                &list_request(
                    Some(cursor),
                    NonZeroU32::new(100).ok_or(DownloadError::Protocol("invalid page size"))?,
                ),
                outgoing,
            )?;
        } else {
            let item = self
                .selected
                .as_ref()
                .ok_or(DownloadError::Protocol("no activity found"))?;
            let request = pull_request(item, self.max_bytes as u64)?;
            self.pull_started = true;
            self.send_rpc(&request, outgoing)?;
        }
        Ok(())
    }

    fn pull_response(
        &mut self,
        result: &wire::PullItemDataResponse,
        outgoing: &mut Vec<Vec<u8>>,
    ) -> Result<(), DownloadError> {
        if result.result != Some(wire::ItemAccessResult::SUCCESS)
            || result.transport != Some(wire::TransportProtocol::MULTILINK_TRANSPORT_PIPE)
        {
            return Err(DownloadError::Protocol(
                "pull refused, result absent, or transport unsupported",
            ));
        }
        self.transfer = Some(
            u16::try_from(
                result
                    .transfer_handle
                    .ok_or(DownloadError::Protocol("transfer handle absent"))?,
            )
            .map_err(|_| DownloadError::Protocol("transfer handle exceeds pipe wire width"))?,
        );
        if !matches!(result.compression_window, Some(0 | 15)) {
            return Err(DownloadError::Protocol(
                "unsupported or absent compression negotiation",
            ));
        }
        self.compression = result.compression_window;
        let (pipe, request) =
            DataChannel::begin(self.client, PIPE_SERVICE, self.mode, self.deadline);
        self.pipe = Some(pipe);
        outgoing.push(request.to_vec());
        Ok(())
    }

    fn transfer_status(
        &mut self,
        status: &wire::TransferStatusRequest,
        peer_request_id: u16,
        outgoing: &mut Vec<Vec<u8>>,
    ) -> Result<(), DownloadError> {
        if self.transfer.is_none() || status.transfer_handle != self.transfer.map(u32::from) {
            return Err(DownloadError::Protocol(
                "status belongs to an unknown transfer",
            ));
        }
        if !status.__buffa_unknown_fields.is_empty() {
            return Err(DownloadError::Protocol("unknown transfer-status fields"));
        }
        if let Some(uid) = status.uid.as_option() {
            let item = self
                .selected
                .as_ref()
                .or_else(|| self.completed.as_ref().map(|file| &file.item))
                .ok_or(DownloadError::Protocol("transfer-status item missing"))?;
            let expected = item
                .uid
                .as_option()
                .ok_or(DownloadError::Protocol("selected item UID absent"))?;
            if (uid.most_significant, uid.least_significant)
                != (expected.most_significant, expected.least_significant)
            {
                return Err(DownloadError::Protocol("transfer-status UID mismatch"));
            }
        }
        if status.failure_reason.is_some() {
            return Err(DownloadError::Protocol("peer reported transfer failure"));
        }
        let bytes = transfer_status_response();
        let limit = usize::from(
            self.identity
                .as_ref()
                .ok_or(DownloadError::Protocol("identity absent"))?
                .max_packet_size,
        )
        .saturating_sub(20);
        self.main.write(
            &encode_frame(&encode_rpc_response(peer_request_id, &bytes, limit)?),
            outgoing,
        )?;
        Ok(())
    }

    fn send_rpc(&mut self, bytes: &[u8], outgoing: &mut Vec<Vec<u8>>) -> Result<(), DownloadError> {
        self.request_id = self.request_id.wrapping_add(1);
        let limit = usize::from(
            self.identity
                .as_ref()
                .ok_or(DownloadError::Protocol("identity absent"))?
                .max_packet_size,
        )
        .saturating_sub(20);
        self.main.write(
            &encode_frame(&encode_rpc_request(self.request_id, bytes, limit)?),
            outgoing,
        )
    }

    fn finish(&mut self) -> Result<(), DownloadError> {
        if !self.pipe_started {
            return Err(DownloadError::Protocol("pipe closed without start"));
        }
        let item = self
            .selected
            .take()
            .ok_or(DownloadError::Protocol("selected item missing"))?;
        let expected = usize::try_from(
            item.data_size
                .ok_or(DownloadError::Protocol("file size absent"))?,
        )
        .map_err(|_| DownloadError::Protocol("file size exceeds host width"))?;
        if expected > self.max_bytes {
            return Err(DownloadError::Protocol("file size exceeds limit"));
        }
        let bytes = if self.compression == Some(15) {
            let mut decoder = Decompress::new(true);
            let mut bytes = Vec::with_capacity(
                expected
                    .checked_add(1)
                    .ok_or(DownloadError::Protocol("file size overflow"))?,
            );
            let status =
                decoder.decompress_vec(&self.compressed, &mut bytes, FlushDecompress::Finish)?;
            if status != Status::StreamEnd || decoder.total_in() != self.compressed.len() as u64 {
                return Err(DownloadError::Protocol(
                    "truncated or trailing compressed data",
                ));
            }
            bytes
        } else {
            std::mem::take(&mut self.compressed)
        };
        if bytes.len() != expected {
            return Err(DownloadError::Protocol("uncompressed file size mismatch"));
        }
        self.compressed.clear();
        self.completed = Some(DownloadedFile { item, bytes });
        Ok(())
    }

    /// Drive ACK/retransmission timers under the whole-operation deadline.
    ///
    /// # Errors
    /// Returns terminal transport, timeout or lifecycle failures.
    pub fn poll(&mut self, now: Duration) -> Result<Vec<Vec<u8>>, DownloadError> {
        let result = self.check(now).and_then(|()| {
            let mut outgoing = Vec::new();
            self.poll_channels(now, &mut outgoing)?;
            Ok(outgoing)
        });
        if result.is_err() {
            self.cancel();
        }
        result
    }

    fn poll_channels(
        &mut self,
        now: Duration,
        outgoing: &mut Vec<Vec<u8>>,
    ) -> Result<(), DownloadError> {
        self.main.poll(now, outgoing)?;
        if let Some(pipe) = &mut self.pipe {
            pipe.poll(now, outgoing)?;
        }
        Ok(())
    }

    /// Take completed bytes only after all outstanding GFDI sends are acknowledged.
    pub fn take_file(&mut self) -> Option<DownloadedFile> {
        if self.main.drained()
            && let Some(file) = self.completed.take()
        {
            self.stopped = true;
            return Some(file);
        }
        None
    }

    /// Observed device identity, if available.
    #[must_use]
    pub fn identity(&self) -> Option<&DeviceInformation> {
        self.identity.as_ref()
    }

    /// Close only known allocated handles that the peer has not already closed.
    #[must_use]
    pub fn close_requests(&self) -> Vec<[u8; 13]> {
        self.pipe
            .as_ref()
            .and_then(DataChannel::close_request)
            .into_iter()
            .chain(self.main.close_request())
            .collect()
    }

    /// Stop locally; the adapter still owns sending cleanup and disconnecting.
    pub fn cancel(&mut self) {
        self.stopped = true;
        self.compressed.clear();
        self.completed = None;
        self.rpc = None;
        if let Some(sender) = &mut self.main.sender {
            sender.cancel();
        }
        if let Some(receiver) = &mut self.main.receiver {
            receiver.close();
        }
        if let Some(pipe) = &mut self.pipe {
            if let Some(sender) = &mut pipe.sender {
                sender.cancel();
            }
            if let Some(receiver) = &mut pipe.receiver {
                receiver.close();
            }
        }
    }

    fn check(&self, now: Duration) -> Result<(), DownloadError> {
        if self.stopped {
            return Err(DownloadError::Protocol("session stopped"));
        }
        if now >= self.deadline {
            return Err(DownloadError::Protocol("download deadline expired"));
        }
        Ok(())
    }
}
