// MAX_STREAM_DATA Frame {
//   Type (i) = 0x11,
//   Stream ID (i),
//   Maximum Stream Data (i),
// }

use crate::{streamid::StreamId, varint::VarInt, SpaceId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaxStreamDataFrame {
    pub stream_id: StreamId,
    pub max_stream_data: VarInt,
}

const MAX_STREAM_DATA_FRAME_TYPE: u8 = 0x11;

impl super::BeFrame for MaxStreamDataFrame {
    fn frame_type(&self) -> super::FrameType {
        super::FrameType::MaxStreamData
    }

    fn belongs_to(&self, space_id: SpaceId) -> bool {
        // __01
        space_id == SpaceId::ZeroRtt || space_id == SpaceId::OneRtt
    }

    fn max_encoding_size(&self) -> usize {
        1 + 8 + 8
    }

    fn encoding_size(&self) -> usize {
        1 + self.stream_id.encoding_size() + self.max_stream_data.encoding_size()
    }
}

pub(super) mod ext {
    use super::{MaxStreamDataFrame, MAX_STREAM_DATA_FRAME_TYPE};

    // nom parser for MAX_STREAM_DATA_FRAME
    pub fn be_max_stream_data_frame(input: &[u8]) -> nom::IResult<&[u8], MaxStreamDataFrame> {
        use crate::{streamid::ext::be_streamid, varint::ext::be_varint};
        use nom::{combinator::map, sequence::pair};
        map(
            pair(be_streamid, be_varint),
            |(stream_id, max_stream_data)| MaxStreamDataFrame {
                stream_id,
                max_stream_data,
            },
        )(input)
    }

    // BufMut write extension for MAX_STREAM_DATA_FRAME
    pub trait WriteMaxStreamDataFrame {
        fn put_max_stream_data_frame(&mut self, frame: &MaxStreamDataFrame);
    }

    impl<T: bytes::BufMut> WriteMaxStreamDataFrame for T {
        fn put_max_stream_data_frame(&mut self, frame: &MaxStreamDataFrame) {
            use crate::streamid::ext::BufMutExt as StreamIdBufMutExt;
            use crate::varint::ext::BufMutExt as VarIntBufMutExt;
            self.put_u8(MAX_STREAM_DATA_FRAME_TYPE);
            self.put_streamid(&frame.stream_id);
            self.put_varint(&frame.max_stream_data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MaxStreamDataFrame, MAX_STREAM_DATA_FRAME_TYPE};
    use crate::varint::VarInt;

    #[test]
    fn test_read_max_stream_data_frame() {
        use super::ext::be_max_stream_data_frame;
        let buf = vec![0x52, 0x34, 0x80, 0, 0x56, 0x78];
        let (_, frame) = be_max_stream_data_frame(&buf).unwrap();
        assert_eq!(frame.stream_id, VarInt(0x1234).into());
        assert_eq!(frame.max_stream_data, VarInt(0x5678));
    }

    #[test]
    fn test_write_max_stream_data_frame() {
        use super::ext::WriteMaxStreamDataFrame;
        let mut buf = Vec::new();
        buf.put_max_stream_data_frame(&MaxStreamDataFrame {
            stream_id: VarInt(0x1234).into(),
            max_stream_data: VarInt(0x5678),
        });
        assert_eq!(
            buf,
            vec![MAX_STREAM_DATA_FRAME_TYPE, 0x52, 0x34, 0x80, 0, 0x56, 0x78]
        );
    }
}
