use std::sync::atomic::{AtomicU32, Ordering};

static GB_SN: AtomicU32 = AtomicU32::new(0);

pub(crate) fn next_sn() -> u32 {
    next_atomic_value(&GB_SN, u32::MAX)
}

fn next_atomic_value(sequence: &AtomicU32, max: u32) -> u32 {
    let mut current = sequence.load(Ordering::Relaxed);
    loop {
        let next = if current >= max { 1 } else { current + 1 };
        match sequence.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(actual) => current = actual,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU32;

    use super::next_atomic_value;

    #[test]
    fn sn_increments_and_wraps_without_zero() {
        let sequence = AtomicU32::new(u32::MAX - 1);
        assert_eq!(next_atomic_value(&sequence, u32::MAX), u32::MAX);
        assert_eq!(next_atomic_value(&sequence, u32::MAX), 1);
        assert_eq!(next_atomic_value(&sequence, u32::MAX), 2);
    }

    #[test]
    fn sn_is_unique_under_concurrency_before_wrap() {
        let sequence = Arc::new(AtomicU32::new(0));
        let handles = (0..8)
            .map(|_| {
                let sequence = Arc::clone(&sequence);
                std::thread::spawn(move || {
                    (0..1_000)
                        .map(|_| next_atomic_value(&sequence, u32::MAX))
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();
        let values = handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("sequence worker"))
            .collect::<Vec<_>>();
        let unique = values.iter().copied().collect::<HashSet<_>>();
        assert_eq!(unique.len(), values.len());
        assert!(!unique.contains(&0));
    }
}
