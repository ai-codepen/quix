use crate::crypto::TlsIO;
use bytes::BytesMut;
use qbase::{
    error::Error,
    frame::ConnectionFrame,
    packet::{
        KeyPhaseToggle, ProtectedHandshakeHeader, ProtectedInitialHeader, ProtectedOneRttHeader,
        ProtectedZeroRTTHeader, SpinToggle,
    },
};
use qrecovery::{
    crypto_stream::{CryptoStreamReader, CryptoStreamWriter},
    rtt::Rtt,
    space::{DataSpace, HandshakeSpace, InitialSpace},
};
use rustls::quic::{KeyChange, Keys, Secrets};
use std::sync::{Arc, Mutex};

/// Key material for use in QUIC packet spaces
///
/// QUIC uses 4 different sets of keys (and progressive key updates for long-running connections):
///
/// * Initial: these can be created from [`Keys::initial()`]
/// * 0-RTT keys: can be retrieved from [`ConnectionCommon::zero_rtt_keys()`]
/// * Handshake: these are returned from [`ConnectionCommon::write_hs()`] after `ClientHello` and
///   `ServerHello` messages have been exchanged
/// * 1-RTT keys: these are returned from [`ConnectionCommon::write_hs()`] after the handshake is done
///
/// Once the 1-RTT keys have been exchanged, either side may initiate a key update. Progressive
/// update keys can be obtained from the [`Secrets`] returned in [`KeyChange::OneRtt`]. Note that
/// only packet keys are updated by key updates; header protection keys remain the same.

/// 所以，先从Keys::initial()获得initial_keys，这是在endpoint层，都可以默认存在的
/// 收到init数据包，用initial_keys去除包头保护，解密包体，写给initial空间，然后从initial空间的crypto流中读出数据，写入
/// 调用write_hs()，获得handshake keys,

/// 收到handshake数据包，用handshake keys去除包头保护，解密包体，写给handshake空间，然后从handshake空间的额crypto流中读出数据，写入
/// 调用write_hs()，获得1-rtt keys，
/// 从ConnectionCommon::zero_rtt_keys()获取zero_rtt_keys,
pub struct Connection {
    tls_session: TlsIO,
    // initial阶段是创建时自带，握手成功之后丢弃
    initial_space: Arc<Mutex<Option<Box<InitialSpace>>>>,
    handshake_space: Arc<Mutex<Option<Box<HandshakeSpace>>>>,

    zero_rtt_keys: Option<Box<Keys>>,
    one_rtt_keys: Option<Keys>,
    one_rtt_secrets: Option<Secrets>,

    data_space: DataSpace,

    // 暂时性的，rtt应该跟path相关
    rtt: Rtt,

    spin: SpinToggle,
    key_phase: KeyPhaseToggle,
}

impl Connection {
    async fn exchange_hs(
        tls_session: TlsIO,
        (stream_reader, stream_writer): (CryptoStreamReader, CryptoStreamWriter),
    ) -> std::io::Result<KeyChange> {
        let (tls_reader, tls_writer) = tls_session.split_io();
        let loop_read = tls_reader.loop_read_from(stream_reader);
        let mut poll_writer = tls_writer.write_to(stream_writer);
        let key_change = poll_writer.loop_write().await?;
        loop_read.end().await?;
        Ok(key_change)
    }
}

impl Connection {
    pub fn receive_initial_packet(
        &mut self,
        header: ProtectedInitialHeader,
        packet: BytesMut,
        pn_offset: usize,
    ) -> Result<(), Error> {
        let mut initial_space = self.initial_space.lock().unwrap();
        if let Some(ref mut space) = *initial_space {}
        // 如果initial space不存在了，说明握手已经彻底完成，不需再对initial数据包进行处理
        Ok(())
    }

    pub fn receive_handshake_packet(
        &mut self,
        header: ProtectedHandshakeHeader,
        packet: BytesMut,
        pn_offset: usize,
    ) -> Result<(), Error> {
        let mut handshake_space = self.handshake_space.lock().unwrap();
        if let Some(ref mut space) = *handshake_space {}
        // 如果handshake space不存在了，说明握手已经彻底完成，不需再对handshake数据包进行处理
        Ok(())
    }

    pub fn receive_zero_rtt_packet(&mut self, header: ProtectedZeroRTTHeader, packet: BytesMut) {
        // todo
    }

    pub fn receive_one_rtt_packet(&mut self, header: ProtectedOneRttHeader, packet: BytesMut) {
        // todo
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4)
    }
}
