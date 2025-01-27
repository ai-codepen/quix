// DATA_BLOCKED Frame {
//   Type (i) = 0x14,
//   Maximum Data (i),
// }

use crate::{varint::VarInt, SpaceId};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DataBlockedFrame {
    pub limit: VarInt,
}

const DATA_BLOCKED_FRAME_TYPE: u8 = 0x14;

impl super::BeFrame for DataBlockedFrame {
    fn frame_type(&self) -> super::FrameType {
        super::FrameType::Crypto
    }

    fn belongs_to(&self, space_id: SpaceId) -> bool {
        // __01
        space_id == SpaceId::ZeroRtt || space_id == SpaceId::OneRtt
    }

    fn max_encoding_size(&self) -> usize {
        1 + 8
    }

    fn encoding_size(&self) -> usize {
        1 + self.limit.encoding_size()
    }
}

pub(super) mod ext {
    use super::{DataBlockedFrame, DATA_BLOCKED_FRAME_TYPE};

    // nom parser for DATA_BLOCKED_FRAME
    pub fn be_data_blocked_frame(input: &[u8]) -> nom::IResult<&[u8], DataBlockedFrame> {
        use crate::varint::ext::be_varint;
        use nom::combinator::map;
        map(be_varint, |limit| DataBlockedFrame { limit })(input)
    }

    // BufMut write extension for DATA_BLOCKED_FRAME
    pub trait WriteDataBlockedFrame {
        fn put_data_blocked_frame(&mut self, frame: &DataBlockedFrame);
    }

    impl<T: bytes::BufMut> WriteDataBlockedFrame for T {
        fn put_data_blocked_frame(&mut self, frame: &DataBlockedFrame) {
            use crate::varint::ext::BufMutExt as VarIntBufMutExt;
            self.put_u8(DATA_BLOCKED_FRAME_TYPE);
            self.put_varint(&frame.limit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DataBlockedFrame, DATA_BLOCKED_FRAME_TYPE};

    #[test]
    fn test_read_data_blocked_frame() {
        use super::ext::be_data_blocked_frame;
        let buf = vec![0x52, 0x34];
        let (_, frame) = be_data_blocked_frame(&buf).unwrap();
        assert_eq!(
            frame,
            DataBlockedFrame {
                limit: crate::varint::VarInt(0x1234)
            }
        );
    }

    #[test]
    fn test_write_data_blocked_frame() {
        use super::ext::WriteDataBlockedFrame;
        let mut buf = Vec::new();
        buf.put_data_blocked_frame(&DataBlockedFrame {
            limit: crate::varint::VarInt(0x1234),
        });
        assert_eq!(buf, vec![DATA_BLOCKED_FRAME_TYPE, 0x52, 0x34]);
    }
}
