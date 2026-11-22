//! WaOS port: WASI Preview 3, through getrandom 0.4, whose backend for it imports `wasi:random`
//! 0.3. getrandom 0.3 knows only Previews 1 and 2.
use crate::Error;
use core::mem::MaybeUninit;

pub use crate::util::{inner_u32, inner_u64};

#[inline]
pub fn fill_inner(dest: &mut [MaybeUninit<u8>]) -> Result<(), Error> {
    getrandom04::fill_uninit(dest)
        .map(|_| ())
        .map_err(|_| Error::UNEXPECTED)
}
