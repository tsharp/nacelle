use crate::error::CascadeError;

pub(crate) fn checked_u32_len(len: usize) -> Result<u32, CascadeError> {
    u32::try_from(len).map_err(|_| CascadeError::FrameTooLarge {
        len,
        max: u32::MAX as usize,
    })
}
