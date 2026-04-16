use crate::error::NacelleError;

pub(crate) fn checked_u32_len(len: usize) -> Result<u32, NacelleError> {
    u32::try_from(len).map_err(|_| NacelleError::FrameTooLarge {
        len,
        max: u32::MAX as usize,
    })
}
