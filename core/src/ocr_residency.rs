//! A short, sequential OCR reuse window for chapter translation.
use std::time::Duration;

const PAGES_PER_BATCH: usize = 4;
const MAX_TRANSLATION_WAIT: Duration = Duration::from_secs(60);

pub(super) fn has_next_in_batch(index: usize, total: usize) -> bool {
    let done = index + 1;
    done < total && !done.is_multiple_of(PAGES_PER_BATCH)
}

#[cfg(target_os = "macos")]
pub(super) fn normal_memory_pressure() -> bool {
    // Darwin reports the DISPATCH_MEMORYPRESSURE_* level. An unavailable or
    // unrecognized reading uses the release-before-translation path.
    let mut level: libc::c_int = 0;
    let mut length = size_of_val(&level);
    let status = unsafe {
        libc::sysctlbyname(
            c"kern.memorystatus_vm_pressure_level".as_ptr(),
            (&raw mut level).cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    status == 0 && length == size_of_val(&level) && level == 1
}

#[cfg(not(target_os = "macos"))]
pub(super) fn normal_memory_pressure() -> bool { false }

pub(super) async fn wait_for_release() {
    // A local language/vision model can start allocating only after it receives
    // the translation request. Recheck while that request is in flight.
    let _ = tokio::time::timeout(MAX_TRANSLATION_WAIT, async {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if !normal_memory_pressure() { break; }
        }
    }).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releases_at_batch_boundaries_and_on_last_page() {
        let policy = |total| (0..total).map(|i| has_next_in_batch(i, total)).collect::<Vec<_>>();
        assert_eq!(policy(1), [false]);
        assert_eq!(policy(2), [true, false]);
        assert_eq!(policy(9), [true, true, true, false, true, true, true, false, false]);
    }
}
