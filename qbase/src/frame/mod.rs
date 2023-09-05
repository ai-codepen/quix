// This folder defines all the frames, including their parsing and packaging processes.

pub mod ack;
pub mod crypto;
pub mod data_blocked;
pub mod max_data;
pub mod max_stream_data;
pub mod max_streams;
pub mod padding;
pub mod ping;
pub mod reset_stream;
pub mod stop_sending;
pub mod stream;
pub mod stream_data_blocked;
pub mod streams_blocked;

use bytes::Bytes;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum FrameType {
    Padding,
    Ping,
    DataBlocked,
    MaxData,
    MaxStreamData,
    MaxStreams(u8),
    StreamDataBlocked,
    Ack(u8),
    Stream(u8),
    ResetStream,
    StopSending,
    Crypto,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct InvalidFrameType(u8);

impl TryFrom<u8> for FrameType {
    type Error = InvalidFrameType;

    fn try_from(frame_type: u8) -> Result<Self, Self::Error> {
        Ok(match frame_type {
            padding::PADDING_FRAME_TYPE => FrameType::Padding,
            ping::PING_FRAME_TYPE => FrameType::Ping,
            0x2 | 0x3 => FrameType::Ack(frame_type & 0b1),
            reset_stream::RESET_STREAM_FRAME_TYPE => FrameType::ResetStream,
            crypto::CRYPTO_FRAME_TYPE => FrameType::Crypto,
            data_blocked::DATA_BLOCKED_FRAME_TYPE => FrameType::DataBlocked,
            max_data::MAX_DATA_FRAME_TYPE => FrameType::MaxData,
            max_stream_data::MAX_STREAM_DATA_FRAME_TYPE => FrameType::MaxStreamData,
            0x12 | 0x13 => FrameType::MaxStreams(frame_type & 0b1),
            stop_sending::STOP_SENDING_FRAME_TYPE => FrameType::StopSending,
            stream_data_blocked::STREAM_DATA_BLOCKED_FRAME_TYPE => FrameType::StreamDataBlocked,
            8..=15 => FrameType::Stream(frame_type & 0b111),
            _ => return Err(InvalidFrameType(frame_type)),
        })
    }
}

/// 读取的Frame，其中涉及数据体的，得用Bytes来保存，因为数据体可能很大，
/// 放在Frame结构中会很占空间，且有copy的性能开销；如果使用`&[u8]`,
/// 则有生命周期的限制，而数据体实际上要等应用读取后，才可以释放，而并非
/// 在读取解析完一个包后才释放。
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ReadFrame {
    Padding(padding::PaddingFrame),
    Ping(ping::PingFrame),
    Ack(ack::AckFrame),
    Stream(stream::StreamFrame, Bytes),
    ResetStream(reset_stream::ResetStreamFrame),
    Crypto(crypto::CryptoFrame, Bytes),
    DataBlocked(data_blocked::DataBlockedFrame),
    MaxData(max_data::MaxDataFrame),
    MaxStreamData(max_stream_data::MaxStreamDataFrame),
    MaxStreams(max_streams::MaxStreamsFrame),
    StreamDataBlocked(stream_data_blocked::StreamDataBlockedFrame),
    StopSending(stop_sending::StopSendingFrame),
}

pub mod ext {
    use super::{
        ack::ext::ack_frame_with_flag, crypto::ext::be_crypto_frame,
        data_blocked::ext::be_data_blocked_frame, max_data::ext::be_max_data_frame,
        max_stream_data::ext::be_max_stream_data_frame,
        max_streams::ext::max_streams_frame_with_dir, padding::ext::be_padding_frame,
        ping::ext::be_ping_frame, reset_stream::ext::be_reset_stream_frame,
        stop_sending::ext::be_stop_sending_frame, stream::ext::stream_frame_with_flag,
        stream_data_blocked::ext::be_stream_data_blocked_frame, FrameType, ReadFrame,
    };

    use bytes::Bytes;
    use nom::{
        combinator::{flat_map, map, map_res},
        error::{Error, ErrorKind},
        Err, IResult,
    };

    fn be_frame_type(input: &[u8]) -> IResult<&[u8], FrameType> {
        use crate::varint::ext::be_varint;
        map_res(be_varint, |frame_type| {
            FrameType::try_from(frame_type.into_inner() as u8)
                .map_err(|_| Error::new(input, ErrorKind::Alt))
        })(input)
    }

    /// Some frames like `STREAM` and `CRYPTO` have a data body, which use `bytes::Bytes` to store.
    fn complete_frame(
        frame_type: FrameType,
        raw: Bytes,
    ) -> impl Fn(&[u8]) -> IResult<&[u8], ReadFrame> {
        move |input: &[u8]| match frame_type {
            FrameType::Padding => map(be_padding_frame, ReadFrame::Padding)(input),
            FrameType::Ping => map(be_ping_frame, ReadFrame::Ping)(input),
            FrameType::Ack(ecn) => map(ack_frame_with_flag(ecn), ReadFrame::Ack)(input),
            FrameType::ResetStream => map(be_reset_stream_frame, ReadFrame::ResetStream)(input),
            FrameType::DataBlocked => map(be_data_blocked_frame, ReadFrame::DataBlocked)(input),
            FrameType::MaxData => map(be_max_data_frame, ReadFrame::MaxData)(input),
            FrameType::StopSending => map(be_stop_sending_frame, ReadFrame::StopSending)(input),
            FrameType::MaxStreamData => {
                map(be_max_stream_data_frame, ReadFrame::MaxStreamData)(input)
            }
            FrameType::MaxStreams(dir) => {
                map(max_streams_frame_with_dir(dir), ReadFrame::MaxStreams)(input)
            }
            FrameType::StreamDataBlocked => {
                map(be_stream_data_blocked_frame, ReadFrame::StreamDataBlocked)(input)
            }
            FrameType::Crypto => {
                let (input, frame) = be_crypto_frame(input)?;
                let start = raw.len() - input.len();
                let len = frame.length.into_inner() as usize;
                if input.len() < len {
                    Err(Err::Incomplete(nom::Needed::new(len - input.len())))
                } else {
                    let data = raw.slice(start..start + len);
                    Ok((&input[len..], ReadFrame::Crypto(frame, data)))
                }
            }
            FrameType::Stream(flag) => {
                let (input, frame) = stream_frame_with_flag(flag)(input)?;
                let start = raw.len() - input.len();
                let len = frame.length;
                if input.len() < len {
                    Err(Err::Incomplete(nom::Needed::new(len - input.len())))
                } else {
                    let data = raw.slice(start..start + len);
                    Ok((&input[len..], ReadFrame::Stream(frame, data)))
                }
            }
        }
    }

    // nom parser for FRAME
    pub fn be_frame<'a>(input: &'a [u8], raw: &Bytes) -> nom::IResult<&'a [u8], ReadFrame> {
        flat_map(be_frame_type, |frame_type| {
            complete_frame(frame_type, raw.clone())
        })(input)
    }

    /*
    pub trait BufMutExt {
        fn put_frame(&mut self, frame: &ReadFrame);
    }

    impl<T: bytes::BufMut> BufMutExt for T {
        fn put_frame(&mut self, frame: &ReadFrame) {
            match frame {
                ReadFrame::Padding(frame) => self.put_padding_frame(frame),
                ReadFrame::Ping(frame) => self.put_ping_frame(frame),
                //Frame::Ack(frame) => self.put_ack_frame(frame),
                ReadFrame::ResetStream(frame) => self.put_reset_stream_frame(frame),
                ReadFrame::Crypto(frame, data) => self.put_crypto_frame(frame, data),
            }
        }
    }
    */
}

#[cfg(test)]
mod tests {

    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}