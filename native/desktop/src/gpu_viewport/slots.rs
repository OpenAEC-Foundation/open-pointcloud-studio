//! Explicit camera/upload ownership for every visible viewport, across windows.
//! The compositor shares primitive storage, so all primitives use one live mask.
pub(super) const CAPACITY: usize = 8;

pub(super) struct ViewSlots<T> {
    slots: [Option<T>; CAPACITY],
}
impl<T> Default for ViewSlots<T> {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| None),
        }
    }
}
impl<T> ViewSlots<T> {
    pub(super) fn prepare(
        &mut self,
        slot: usize,
        live: u8,
        create: impl FnOnce() -> T,
    ) -> Option<&mut T> {
        for (index, value) in self.slots.iter_mut().enumerate() {
            if live & (1 << index) == 0 {
                *value = None;
            }
        }
        if slot >= CAPACITY || live & (1 << slot) == 0 {
            return None;
        }
        Some(self.slots[slot].get_or_insert_with(create))
    }
    pub(super) fn get(&self, slot: usize) -> Option<&T> {
        self.slots.get(slot)?.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    #[test]
    fn eight_views_keep_their_prepared_cameras_independent() {
        let mut slots = ViewSlots::default();
        for index in 0..CAPACITY {
            *slots.prepare(index, u8::MAX, || index).unwrap() = index + 10;
        }
        for index in 0..CAPACITY {
            assert_eq!(slots.get(index), Some(&(index + 10)));
        }
        *slots
            .prepare(3, u8::MAX, || {
                panic!("an existing view must reuse its resources")
            })
            .unwrap() = 99;
        assert_eq!(slots.get(3), Some(&99));
        assert_eq!(slots.get(2), Some(&12));
    }
    #[test]
    fn closing_a_view_releases_its_resources_and_reuses_the_slot() {
        struct Resource(Arc<AtomicUsize>);
        impl Drop for Resource {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicUsize::new(0));
        let mut slots = ViewSlots::default();
        slots
            .prepare(0, 3, || Resource(Arc::clone(&dropped)))
            .unwrap();
        slots
            .prepare(1, 3, || Resource(Arc::clone(&dropped)))
            .unwrap();
        slots
            .prepare(0, 1, || panic!("the remaining view must survive"))
            .unwrap();
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        assert!(slots.get(1).is_none());
        slots
            .prepare(1, 3, || Resource(Arc::clone(&dropped)))
            .unwrap();
        drop(slots);
        assert_eq!(dropped.load(Ordering::SeqCst), 3);
    }
    #[test]
    fn inactive_or_invalid_slots_never_allocate_or_panic() {
        let mut slots = ViewSlots::<usize>::default();
        assert!(slots
            .prepare(CAPACITY, u8::MAX, || panic!("invalid slot allocated"))
            .is_none());
        assert!(slots
            .prepare(0, 0, || panic!("inactive slot allocated"))
            .is_none());
        assert!(slots.get(CAPACITY).is_none());
    }
}
