#[cfg(feature = "alloc")]
pub(crate) fn filled_vec<T: Clone>(
    len: usize,
    value: T,
) -> crate::error::Result<alloc::vec::Vec<T>> {
    let mut storage = alloc::vec::Vec::new();
    storage.try_reserve_exact(len)?;
    storage.resize(len, value);
    Ok(storage)
}

#[cfg(all(test, feature = "alloc"))]
mod tests {
    #[test]
    fn unrepresentable_allocation_is_an_error() {
        assert!(matches!(
            super::filled_vec::<u8>(usize::MAX, 0),
            Err(crate::Error::Allocation(_))
        ));
    }
}
