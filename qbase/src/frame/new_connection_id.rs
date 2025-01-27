// NEW_CONNECTION_ID Frame {
//   Type (i) = 0x18,
//   Sequence Number (i),
//   Retire Prior To (i),
//   Length (8),
//   Connection ID (8..160),
//   Stateless Reset Token (128),
// }

use crate::{
    cid::{ConnectionId, ResetToken},
    varint::VarInt,
    SpaceId,
};

const NEW_CONNECTION_ID_FRAME_TYPE: u8 = 0x18;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewConnectionIdFrame {
    pub sequence: VarInt,
    pub retire_prior_to: VarInt,
    pub id: ConnectionId,
    pub reset_token: ResetToken,
}

impl super::BeFrame for NewConnectionIdFrame {
    fn frame_type(&self) -> super::FrameType {
        super::FrameType::NewConnectionId
    }

    fn belongs_to(&self, space_id: SpaceId) -> bool {
        // __01
        space_id == SpaceId::ZeroRtt || space_id == SpaceId::OneRtt
    }

    fn encoding_size(&self) -> usize {
        todo!()
    }

    fn max_encoding_size(&self) -> usize {
        todo!()
    }
}

pub(super) mod ext {
    use super::NewConnectionIdFrame;
    use crate::{
        cid::{ResetToken, WriteConnectionId, RESET_TOKEN_SIZE},
        varint::ext::{be_varint, BufMutExt},
    };

    pub fn be_new_connection_id_frame(input: &[u8]) -> nom::IResult<&[u8], NewConnectionIdFrame> {
        use nom::bytes::streaming::take;
        use nom::number::streaming::be_u8;
        let (remain, sequence) = be_varint(input)?;
        let (remain, retire_prior_to) = be_varint(remain)?;
        // The value in the Retire Prior To field MUST be less than or equal to the value in the
        // Sequence Number field. Receiving a value in the Retire Prior To field that is greater
        // than that in the Sequence Number field MUST be treated as a connection error of type
        // FRAME_ENCODING_ERROR.
        if retire_prior_to > sequence {
            return Err(nom::Err::Error(nom::error::make_error(
                input,
                nom::error::ErrorKind::Verify,
            )));
        }
        let (reamin, length) = be_u8(remain)?;
        if length > crate::cid::MAX_CID_SIZE as u8 || length == 0 {
            return Err(nom::Err::Error(nom::error::make_error(
                input,
                nom::error::ErrorKind::Verify,
            )));
        }
        let (remain, id) = super::ConnectionId::from_buf(reamin, length as usize)?;
        let (remain, reset_token) = take(RESET_TOKEN_SIZE)(remain)?;
        Ok((
            remain,
            NewConnectionIdFrame {
                sequence,
                retire_prior_to,
                id,
                reset_token: ResetToken::new_with(reset_token),
            },
        ))
    }

    pub trait WriteNewConnectionIdFrame {
        fn put_new_connection_id_frame(&mut self, frame: &NewConnectionIdFrame);
    }

    impl<T: bytes::BufMut> WriteNewConnectionIdFrame for T {
        fn put_new_connection_id_frame(&mut self, frame: &NewConnectionIdFrame) {
            self.put_u8(super::NEW_CONNECTION_ID_FRAME_TYPE);
            self.put_varint(&frame.sequence);
            self.put_varint(&frame.retire_prior_to);
            self.put_connection_id(&frame.id);
            self.put_slice(&frame.reset_token);
        }
    }
}
