use super::{
    crypto::{CryptoStream, TransmitCrypto},
    index_deque::IndexDeque,
    rtt::Rtt,
    streams::{NoStreams, Streams, TransmitStream},
};
use bytes::{BufMut, Bytes};
use qbase::{
    error::Error,
    frame::{ext::*, *},
    varint::{VarInt, VARINT_MAX},
    SpaceId,
};
use std::{
    collections::VecDeque,
    fmt::Debug,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub enum SpaceFrame {
    Ack(AckFrame, Arc<Mutex<Rtt>>),
    Stream(StreamCtlFrame),
    Data(DataFrame, Bytes),
}

pub trait TrySend {
    type Buffer: BufMut;

    fn try_send(&mut self, buf: &mut Self::Buffer) -> Result<Option<(u64, usize)>, Error>;
}

/// When a network socket receives a data packet and determines that it belongs
/// to a specific space, the content of the packet is passed on to that space.
pub trait Receive {
    fn expected_pn(&self) -> u64;

    fn record(&self, pktid: u64, is_ack_eliciting: bool);

    fn recv_frame(&self, frame: SpaceFrame) -> Result<(), Error>;
}

#[derive(Debug, Clone)]
enum Record {
    Pure(PureFrame),
    Data(DataFrame),
    Ack(AckRecord),
}

type Payload = Vec<Record>;

#[derive(Debug, Clone, Default)]
enum State {
    #[default]
    NotReceived,
    // aka NACK: negative acknowledgment or not acknowledged,
    //     indicate that data transmitted over a network was received
    //     with errors or was otherwise unreadable.
    Unreached,
    Ignored(Instant),
    Important(Instant),
    Synced(Instant),
}

impl State {
    fn new_rcvd(t: Instant, is_ack_eliciting: bool) -> Self {
        if is_ack_eliciting {
            Self::Important(t)
        } else {
            Self::Ignored(t)
        }
    }

    fn has_rcvd(&self) -> bool {
        matches!(
            self,
            Self::Ignored(_) | Self::Important(_) | Self::Synced(_)
        )
    }

    fn has_not_rcvd(&self) -> bool {
        matches!(self, Self::NotReceived | Self::Unreached)
    }

    fn delay(&self) -> Option<Duration> {
        match self {
            Self::Ignored(t) | Self::Important(t) | Self::Synced(t) => Some(t.elapsed()),
            _ => None,
        }
    }

    fn be_synced(&mut self) {
        match self {
            Self::Ignored(t) | Self::Important(t) => {
                *self = Self::Synced(*t);
            }
            Self::NotReceived => *self = Self::Unreached,
            _ => (),
        }
    }
}

#[derive(Debug, Clone)]
struct Packet {
    send_time: Instant,
    payload: Payload,
    sent_bytes: usize,
    is_ack_eliciting: bool,
}

const PACKET_THRESHOLD: u64 = 3;

/// 可靠空间的抽象实现，需要实现上述所有trait
/// 可靠空间中的重传、确认，由可靠空间内部实现，无需外露
#[derive(Debug)]
struct Space<CT, ST>
where
    CT: TransmitCrypto,
    ST: TransmitStream,
{
    space_id: SpaceId,
    // 将要发出的数据帧，包括重传的数据帧；可以是外部的功能帧，也可以是具体传输空间内部的
    // 起到“信号”作用的信令帧，比如数据空间内部的各类通信帧。
    // 需要注意的是，数据帧以及Ack帧(记录)，并不在此中保存，因为数据帧占数据空间，ack帧
    // 则是内部可靠性的产物，他们在发包记录中会作记录保存。
    frames: Arc<Mutex<VecDeque<PureFrame>>>,
    // 记录着发包时间、发包内容，供收到ack frame时，确认那些内容被接收了，哪些丢失了，需要
    // 重传。如果是一般帧，直接进入帧队列就可以了，但有2种需要特殊处理：
    // - 数据帧记录：无论被确认还是判定丢失了，都要通知发送缓冲区
    // - ack帧记录：被确认了，要滑动ack记录队列到合适位置
    // 另外，因为发送信令帧，是自动重传的，因此无需其他实现干扰
    inflight_packets: IndexDeque<Option<Packet>, VARINT_MAX>,
    disorder_tolerance: u64,
    time_of_last_sent_ack_eliciting_packet: Option<Instant>,
    largest_acked_pktid: Option<u64>,
    // 设计丢包重传定时器，在收到AckFrame的探测丢包时，可能会设置该定时器，实际上是过期时间
    loss_time: Option<Instant>,

    // 用于产生ack frame，Instant用于计算ack_delay，bool表明是否ack eliciting
    rcvd_packets: IndexDeque<State, VARINT_MAX>,
    // 收到的最大的ack-eliciting packet的pktid
    largest_rcvd_ack_eliciting_pktid: u64,
    last_synced_ack_largest: u64,
    new_lost_event: bool,
    rcvd_unreached_packet: bool,
    // 下一次需要同步ack frame的时间：
    // - 每次发送ack frame后，会重置该时间为None
    // - 每次收到新的ack-eliciting frame后，会更新该时间
    time_to_sync: Option<Instant>,
    // 应该计算rtt的时候，传进来；或者收到ack frame的时候，将(last_rtt, ack_delay)传出去
    max_ack_delay: Duration,

    stm_trans: ST,
    tls_trans: CT,
}

impl<CT, ST> Space<CT, ST>
where
    CT: TransmitCrypto,
    ST: TransmitStream,
{
    pub(crate) fn build(space_id: SpaceId, tls_transmission: CT, streams_transmission: ST) -> Self {
        Self {
            space_id,
            frames: Arc::new(Mutex::new(VecDeque::new())),
            inflight_packets: IndexDeque::new(),
            disorder_tolerance: 0,
            time_of_last_sent_ack_eliciting_packet: None,
            largest_acked_pktid: None,
            loss_time: None,
            rcvd_packets: IndexDeque::new(),
            largest_rcvd_ack_eliciting_pktid: 0,
            last_synced_ack_largest: 0,
            new_lost_event: false,
            rcvd_unreached_packet: false,
            time_to_sync: None,
            max_ack_delay: Duration::from_millis(25),
            stm_trans: streams_transmission,
            tls_trans: tls_transmission,
        }
    }

    pub fn space_id(&self) -> SpaceId {
        self.space_id
    }

    pub fn write_frame(&mut self, frame: PureFrame) {
        assert!(frame.belongs_to(self.space_id));
        let mut frames = self.frames.lock().unwrap();
        frames.push_back(frame);
    }

    fn confirm(&mut self, payload: Payload) {
        for record in payload {
            match record {
                Record::Ack(ack) => {
                    let _ = self
                        .rcvd_packets
                        .drain_to(ack.0.saturating_sub(self.disorder_tolerance));
                }
                Record::Pure(_frame) => {
                    todo!("哪些帧需要确认呢？")
                }
                Record::Data(data) => match data {
                    DataFrame::Crypto(f) => self.tls_trans.confirm_data(f),
                    DataFrame::Stream(f) => self.stm_trans.confirm_data(f),
                },
            }
        }
    }

    fn expected_pn(&self) -> u64 {
        self.rcvd_packets.largest()
    }

    fn recv_frame(&mut self, frame: SpaceFrame) -> Result<(), Error> {
        match frame {
            SpaceFrame::Ack(ack, rtt) => {
                let _ = self.recv_ack_frame(ack, rtt);
            }
            SpaceFrame::Stream(f) => self.stm_trans.recv_frame(f)?,
            SpaceFrame::Data(f, data) => match f {
                DataFrame::Crypto(f) => self.tls_trans.recv_data(f, data)?,
                DataFrame::Stream(f) => self.stm_trans.recv_data(f, data)?,
            },
        };
        Ok(())
    }

    fn record(&mut self, pkt_id: u64, is_ack_eliciting: bool) {
        self.rcvd_packets
            .insert(pkt_id, State::new_rcvd(Instant::now(), is_ack_eliciting))
            .unwrap();
        if is_ack_eliciting {
            if self.largest_rcvd_ack_eliciting_pktid < pkt_id {
                self.largest_rcvd_ack_eliciting_pktid = pkt_id;
                self.new_lost_event |= self
                    .rcvd_packets
                    .iter_with_idx()
                    .rev()
                    .skip_while(|(pn, _)| pn >= &pn)
                    .skip(PACKET_THRESHOLD as usize)
                    .take_while(|(pn, _)| pn > &self.last_synced_ack_largest)
                    .any(|(_, s)| matches!(s, State::NotReceived));
            }
            if pkt_id < self.last_synced_ack_largest {
                self.rcvd_unreached_packet = true;
            }
            self.time_to_sync = self
                .time_to_sync
                .or(Some(Instant::now() + self.max_ack_delay));
        }
    }

    fn gen_ack_frame(&mut self) -> AckFrame {
        // must be a reliable space; otherwise, if it is an unreliable space,
        // such as the 0-RTT space, never send an ACK frame.
        assert!(self.space_id != SpaceId::ZeroRtt);
        // There must be an ACK-eliciting packet; otherwise, it will not
        // trigger the sending of an ACK frame.
        debug_assert!(self
            .rcvd_packets
            .iter()
            .any(|p| matches!(p, State::Important(_))));

        let largest = self.rcvd_packets.offset() + self.rcvd_packets.len() as u64 - 1;
        let delay = self.rcvd_packets.get_mut(largest).unwrap().delay().unwrap();
        let mut rcvd_iter = self.rcvd_packets.iter_mut().rev();
        let first_range = rcvd_iter.by_ref().take_while(|s| s.has_rcvd()).count() - 1;
        let mut ranges = Vec::with_capacity(16);
        loop {
            if rcvd_iter.next().is_none() {
                break;
            }
            let gap = rcvd_iter.by_ref().take_while(|s| s.has_not_rcvd()).count();

            if rcvd_iter.next().is_none() {
                break;
            }
            let acked = rcvd_iter.by_ref().take_while(|s| s.has_rcvd()).count();

            ranges.push(unsafe {
                (
                    VarInt::from_u64_unchecked(gap as u64),
                    VarInt::from_u64_unchecked(acked as u64),
                )
            });
        }

        AckFrame {
            largest: unsafe { VarInt::from_u64_unchecked(largest) },
            delay: unsafe { VarInt::from_u64_unchecked(delay.as_micros() as u64) },
            first_range: unsafe { VarInt::from_u64_unchecked(first_range as u64) },
            ranges,
            // TODO: support ECN
            ecn: None,
        }
    }

    fn recv_ack_frame(&mut self, mut ack: AckFrame, rtt: Arc<Mutex<Rtt>>) -> Option<usize> {
        let largest_acked = ack.largest.into_inner();
        if self
            .largest_acked_pktid
            .map(|v| v > largest_acked)
            .unwrap_or(false)
        {
            return None;
        }
        // largest_acked == self.largest_acked_packet is also acceptable,
        // perhaps indicating that old 'lost' packets have been acknowledged.
        self.largest_acked_pktid = Some(largest_acked);

        let mut no_newly_acked = true;
        let mut includes_ack_eliciting = false;
        let mut acked_bytes = 0;
        let ecn_in_ack = ack.take_ecn();
        let ack_delay = Duration::from_micros(ack.delay.into_inner());
        for range in ack.into_iter() {
            for pktid in range {
                if let Some(packet) = self
                    .inflight_packets
                    .get_mut(pktid)
                    .and_then(|record| record.take())
                {
                    no_newly_acked = false;
                    if packet.is_ack_eliciting {
                        includes_ack_eliciting = true;
                    }
                    self.confirm(packet.payload);
                    acked_bytes += packet.sent_bytes;
                }
            }
        }

        if no_newly_acked {
            return None;
        }

        if let Some(_ecn) = ecn_in_ack {
            todo!("处理ECN信息");
        }

        if let Some(packet) = self
            .inflight_packets
            .get_mut(largest_acked)
            .and_then(|record| record.take())
        {
            if packet.is_ack_eliciting {
                includes_ack_eliciting = true;
            }
            if includes_ack_eliciting {
                let is_handshake_confirmed = self.space_id == SpaceId::OneRtt;
                rtt.lock().unwrap().update(
                    packet.send_time.elapsed(),
                    ack_delay,
                    is_handshake_confirmed,
                );
            }
            self.confirm(packet.payload);
            acked_bytes += packet.sent_bytes;
        }

        // retranmission
        for packet in self
            .inflight_packets
            .drain_to(largest_acked.saturating_sub(PACKET_THRESHOLD))
            .flatten()
        {
            acked_bytes += packet.sent_bytes;
            for record in packet.payload {
                match record {
                    Record::Ack(_) => { /* needn't resend */ }
                    Record::Pure(frame) => {
                        let mut frames = self.frames.lock().unwrap();
                        frames.push_back(frame);
                    }
                    Record::Data(data) => match data {
                        DataFrame::Crypto(f) => self.tls_trans.may_loss_data(f),
                        DataFrame::Stream(f) => self.stm_trans.may_loss_data(f),
                    },
                }
            }
        }

        let loss_delay = rtt.lock().unwrap().loss_delay();
        // Packets sent before this time are deemed lost too.
        let lost_send_time = Instant::now() - loss_delay;
        self.loss_time = None;
        for packet in self
            .inflight_packets
            .iter_mut()
            .take(PACKET_THRESHOLD as usize)
            .filter(|p| p.is_some())
        {
            let send_time = packet.as_ref().unwrap().send_time;
            if send_time <= lost_send_time {
                for record in packet.take().unwrap().payload {
                    match record {
                        Record::Ack(_) => { /* needn't resend */ }
                        Record::Pure(frame) => {
                            let mut frames = self.frames.lock().unwrap();
                            frames.push_back(frame);
                        }
                        Record::Data(data) => match data {
                            DataFrame::Crypto(f) => self.tls_trans.may_loss_data(f),
                            DataFrame::Stream(f) => self.stm_trans.may_loss_data(f),
                        },
                    }
                }
            } else {
                self.loss_time = self
                    .loss_time
                    .map(|t| std::cmp::min(t, send_time + loss_delay))
                    .or(Some(send_time + loss_delay));
            }
        }
        // A small optimization would be to slide forward if the first consecutive
        // packets in the inflight_packets queue have been acknowledged.
        let n = self
            .inflight_packets
            .iter()
            .take_while(|p| p.is_none())
            .count();
        let _ = self.inflight_packets.drain(..n);
        Some(acked_bytes)
    }

    fn need_send_ack_frame(&self) -> bool {
        // non-reliable space such as 0-RTT space, never send ack frame
        if self.space_id == SpaceId::ZeroRtt {
            return false;
        }

        // In order to assist loss detection at the sender, an endpoint SHOULD generate
        // and send an ACK frame without delay when it receives an ack-eliciting packet either:
        //   (下述第一条，莫非是只要ack-eliciting包乱序就要发送ack frame？不至于吧)
        //   (应该是过往ack过的包，里面没被确认的，突然被收到的话，就立刻发ack帧，为避免发送端不必要的重传，这样比较合适)
        // - when the received packet has a packet number less than another
        //   ack-eliciting packet that has been received, or
        //   (下述这一条比较科学，收包收到感知到丢包，立即发送ack帧）
        // - when the packet has a packet number larger than the highest-numbered
        //   ack-eliciting packet that has been received and there are missing
        //   packets between that packet and this packet.
        if self.new_lost_event || self.rcvd_unreached_packet {
            return true;
        }

        // ack-eliciting packets MUST be acknowledged at least once within the maximum delay
        match self.time_to_sync {
            Some(t) => t > Instant::now(),
            None => false,
        }
    }
}

impl<CT, ST> TrySend for Space<CT, ST>
where
    CT: TransmitCrypto<Buffer = bytes::BytesMut>,
    ST: TransmitStream<Buffer = bytes::BytesMut>,
{
    type Buffer = bytes::BytesMut;

    fn try_send(&mut self, buf: &mut Self::Buffer) -> Result<Option<(u64, usize)>, Error> {
        let mut is_ack_eliciting = false;
        let mut remaning = buf.remaining_mut();
        let mut payload = Payload::new();
        if self.need_send_ack_frame() {
            let ack = self.gen_ack_frame();
            if remaning >= ack.max_encoding_size() || remaning >= ack.encoding_size() {
                self.time_to_sync = None;
                self.new_lost_event = false;
                self.rcvd_unreached_packet = false;
                self.last_synced_ack_largest = ack.largest.into_inner();
                buf.put_ack_frame(&ack);
                payload.push(Record::Ack(ack.into()));
                // The ACK frame is not counted towards sent_bytes, is not subject to
                // amplification attacks, and is not subject to flow control limitations.
                remaning = buf.remaining_mut();
                // All known packet information needs to be marked as synchronized.
                self.rcvd_packets.iter_mut().for_each(|s| s.be_synced());
            }
        }

        // Prioritize retransmitting lost or info frames.
        loop {
            let mut frames = self.frames.lock().unwrap();
            if let Some(frame) = frames.front() {
                if remaning >= frame.max_encoding_size() || remaning >= frame.encoding_size() {
                    buf.put_frame(frame);
                    remaning = buf.remaining_mut();
                    is_ack_eliciting = true;

                    let frame = frames.pop_front().unwrap();
                    payload.push(Record::Pure(frame));
                    continue;
                } else {
                    break;
                }
            }
        }

        // Consider transmit stream info frames if has
        if let Some((stream_info_frame, _len)) = self.stm_trans.try_send_frame(buf) {
            payload.push(Record::Pure(PureFrame::Stream(stream_info_frame)));
        }

        // Consider transmitting data frames.
        if self.space_id != SpaceId::ZeroRtt {
            while let Some((data_frame, ignore)) = self.tls_trans.try_send_data(buf) {
                payload.push(Record::Data(DataFrame::Crypto(data_frame)));
                remaning += ignore;
            }
        }
        while let Some((data_frame, _)) = self.stm_trans.try_send_data(buf) {
            payload.push(Record::Data(DataFrame::Stream(data_frame)));
        }

        // Record
        let sent_bytes = remaning - buf.remaining_mut();
        if sent_bytes == 0 {
            // no data to send
            return Ok(None);
        }
        if is_ack_eliciting {
            self.time_of_last_sent_ack_eliciting_packet = Some(Instant::now());
        }
        let pktid = self.inflight_packets.push(Some(Packet {
            send_time: Instant::now(),
            payload,
            sent_bytes,
            is_ack_eliciting,
        }))?;
        Ok(Some((pktid, sent_bytes)))
    }
}

/// 为何Space需要时Arc的？因为Space既要收取数据，也要发送数据，而收发是独立的行为，因此要用Arc包裹。
/// 对于InitialSpace和HandshakeSpace，十分适用ArcSpace
type ArcSpace<CT, ST> = Arc<Mutex<Space<CT, ST>>>;

#[derive(Debug)]
pub struct SpaceIO<CT, ST>(ArcSpace<CT, ST>)
where
    CT: TransmitCrypto,
    ST: TransmitStream;

impl SpaceIO<CryptoStream, NoStreams> {
    pub fn new_initial(crypto_stream: CryptoStream) -> Self {
        Self(Arc::new(Mutex::new(Space::build(
            SpaceId::Initial,
            crypto_stream,
            NoStreams,
        ))))
    }

    pub fn new_handshake(crypto_stream: CryptoStream) -> Self {
        Self(Arc::new(Mutex::new(Space::build(
            SpaceId::Handshake,
            crypto_stream,
            NoStreams,
        ))))
    }
}

/// Data space, initially it's a 0RTT space, and later it needs to be upgraded to a 1RTT space.
/// The data in the 0RTT space is unreliable and cannot transmit CryptoFrame. It is constrained
/// by the space_id when sending, and a judgment is also made in the task of receiving and unpacking.
/// Therefore, when upgrading, just change the space_id to 1RTT, no other operations are needed.
impl SpaceIO<CryptoStream, Streams> {
    pub fn new(crypto_stream: CryptoStream, streams: Streams) -> Self {
        Self(Arc::new(Mutex::new(Space::build(
            SpaceId::ZeroRtt,
            crypto_stream,
            streams,
        ))))
    }

    pub fn upgrade(&mut self) {
        let mut ds = self.0.lock().unwrap();
        assert_eq!(ds.space_id, SpaceId::ZeroRtt);
        ds.space_id = SpaceId::OneRtt;
    }
}

impl<CT, ST> Receive for SpaceIO<CT, ST>
where
    CT: TransmitCrypto,
    ST: TransmitStream,
{
    fn expected_pn(&self) -> u64 {
        self.0.lock().unwrap().expected_pn()
    }

    fn record(&self, pkt_id: u64, is_ack_eliciting: bool) {
        self.0.lock().unwrap().record(pkt_id, is_ack_eliciting);
    }

    fn recv_frame(&self, frame: SpaceFrame) -> Result<(), Error> {
        self.0.lock().unwrap().recv_frame(frame)
    }
}

impl<CT, ST> Clone for SpaceIO<CT, ST>
where
    CT: TransmitCrypto,
    ST: TransmitStream,
{
    fn clone(&self) -> Self {
        SpaceIO(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
