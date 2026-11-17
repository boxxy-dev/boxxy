//! Sound, thread-guarded typed data storage on [`glib::Object`] instances.

use gtk4::glib;
use gtk4::glib::thread_guard::ThreadGuard;
use std::any::Any;
use std::cell::{Ref, RefCell};
use std::collections::HashMap;

/// Namespace prefix isolating `ObjectExtSafe` keys from GTK/GLib/Adwaita internal C quarks.
const QUARK_NAMESPACE: &str = "boxxy-core-widgets::safe-data::";

/// Upper bound on cached dynamic keys per thread so arbitrary runtime strings cannot grow
/// the thread-local lookup map without bound.
const MAX_CACHED_QUARKS: usize = 512;

type SafeSlot = ThreadGuard<RefCell<Option<Box<dyn Any>>>>;

thread_local! {
    static QUARK_CACHE: RefCell<HashMap<Box<str>, glib::Quark>> = RefCell::new(HashMap::new());
}

#[inline]
fn resolve_quark(key: &str) -> glib::Quark {
    QUARK_CACHE.with(|cache| {
        if let Some(&quark) = cache.borrow().get(key) {
            return quark;
        }

        let mut namespaced = String::with_capacity(QUARK_NAMESPACE.len() + key.len());
        namespaced.push_str(QUARK_NAMESPACE);
        namespaced.push_str(key);
        let quark = glib::Quark::from_str(&namespaced);

        let mut map = cache.borrow_mut();
        if map.len() < MAX_CACHED_QUARKS {
            map.insert(key.into(), quark);
        }
        quark
    })
}

/// Memory-safe, thread-checked wrapper for attaching arbitrary Rust values to `GObject`s.
///
/// # Soundness Guarantees
/// Raw [`glib::object::ObjectExt::set_qdata`] / [`glib::object::ObjectExt::qdata`] stores an
/// untyped pointer without borrow tracking or thread affinity checks. `ObjectExtSafe` closes
/// all three soundness holes:
/// 1. **Runtime type verification (`Any`):** Mismatched `T` queries return `None` instead of
///    transmuting pointers.
/// 2. **Write-once slot + `RefCell` borrow tracking:** Each namespaced quark points to a
///    heap-stable `ThreadGuard<RefCell<Option<Box<dyn Any>>>>` that is never replaced or
///    freed until `GObject` finalization. Mutating or taking the same key while a `Ref<'_, T>`
///    guard from [`ObjectExtSafe::get_safe_data`] is live deterministically panics instead of
///    causing use-after-free.
/// 3. **Thread affinity (`ThreadGuard`):** Accessing or dropping `!Send` payloads (`Rc`, GTK
///    widgets) from a thread other than the creator panics safely instead of racing.
pub trait ObjectExtSafe: glib::object::ObjectExt {
    /// Stores `data` under `key`, replacing any previous value stored under `key`.
    ///
    /// # Panics
    /// Panics if a [`Ref`] returned by [`ObjectExtSafe::get_safe_data`] for the same `key` on
    /// `self` is still alive, or if `key` was originally populated on a different thread.
    fn set_safe_data<T: 'static>(&self, key: &str, data: T) {
        let quark = resolve_quark(key);
        let boxed: Box<dyn Any> = Box::new(data);

        // SAFETY: `quark` is exclusively minted under `QUARK_NAMESPACE` and only ever populated
        // with `SafeSlot` below; we never call `steal_qdata` or overwrite an existing `SafeSlot`.
        if let Some(slot_ptr) = unsafe { self.qdata::<SafeSlot>(quark) } {
            let slot = unsafe { slot_ptr.as_ref() };
            // Drop the `RefMut` borrow *before* dropping `prev` so re-entrant access inside
            // `OldT::drop` cannot trigger a `BorrowMutError`.
            let prev = slot.get_ref().borrow_mut().replace(boxed);
            drop(prev);
        } else {
            let slot: SafeSlot = ThreadGuard::new(RefCell::new(Some(boxed)));
            unsafe {
                self.set_qdata(quark, slot);
            }
        }
    }

    /// Borrows the value of type `T` stored under `key`, returning `None` if absent or if the
    /// stored value has a different concrete type.
    ///
    /// # Panics
    /// Panics if `key` was originally populated on a different thread.
    fn get_safe_data<T: 'static>(&self, key: &str) -> Option<Ref<'_, T>> {
        let quark = resolve_quark(key);
        // SAFETY: `quark` under `QUARK_NAMESPACE` exclusively holds a `SafeSlot` whose outer
        // allocation lives until `self` finalizes (guaranteed to outlive `&self`).
        let slot_ptr = unsafe { self.qdata::<SafeSlot>(quark) }?;
        let slot = unsafe { slot_ptr.as_ref() };
        let guard = slot.get_ref().try_borrow().ok()?;
        Ref::filter_map(guard, |opt| opt.as_deref()?.downcast_ref::<T>()).ok()
    }

    /// Removes and returns the value of type `T` stored under `key`.
    ///
    /// If `key` holds a value of a *different* concrete type, returns `None` while leaving
    /// the stored value intact.
    ///
    /// # Panics
    /// Panics if a [`Ref`] returned by [`ObjectExtSafe::get_safe_data`] for the same `key` on
    /// `self` is still alive, or if `key` was originally populated on a different thread.
    fn steal_safe_data<T: 'static>(&self, key: &str) -> Option<T> {
        let quark = resolve_quark(key);
        // SAFETY: `quark` under `QUARK_NAMESPACE` exclusively holds a `SafeSlot`.
        let slot_ptr = unsafe { self.qdata::<SafeSlot>(quark) }?;
        let slot = unsafe { slot_ptr.as_ref() };
        let mut guard = slot.get_ref().borrow_mut();
        if guard.as_deref().is_some_and(<dyn Any + 'static>::is::<T>) {
            let boxed = guard.take()?;
            drop(guard);
            boxed.downcast::<T>().ok().map(|b| *b)
        } else {
            None
        }
    }
}

impl<T: glib::object::ObjectExt> ObjectExtSafe for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk4::gio;
    use std::rc::Rc;

    #[test]
    fn test_round_trip_overwrite_and_steal() {
        let obj = glib::Object::new::<glib::Object>();
        assert!(obj.get_safe_data::<u64>("counter").is_none());

        obj.set_safe_data("counter", 42_u64);
        assert_eq!(*obj.get_safe_data::<u64>("counter").unwrap(), 42);

        // Overwrite with new value of the same type
        obj.set_safe_data("counter", 99_u64);
        assert_eq!(*obj.get_safe_data::<u64>("counter").unwrap(), 99);

        // Steal removes the value
        assert_eq!(obj.steal_safe_data::<u64>("counter"), Some(99));
        assert!(obj.get_safe_data::<u64>("counter").is_none());
        assert_eq!(obj.steal_safe_data::<u64>("counter"), None);
    }

    #[test]
    fn test_mismatched_type_is_safe_and_non_destructive() {
        let obj = glib::Object::new::<glib::Object>();
        obj.set_safe_data("slot", 1234_u64);

        // Querying with the wrong type returns `None` instead of UB
        assert!(obj.get_safe_data::<String>("slot").is_none());
        // Attempting to steal with the wrong type returns `None` AND preserves the original value
        assert_eq!(obj.steal_safe_data::<String>("slot"), None);
        assert_eq!(*obj.get_safe_data::<u64>("slot").unwrap(), 1234);

        // Overwriting with a completely different type works cleanly
        obj.set_safe_data("slot", String::from("replaced"));
        assert!(obj.get_safe_data::<u64>("slot").is_none());
        assert_eq!(
            obj.get_safe_data::<String>("slot").unwrap().as_str(),
            "replaced"
        );
    }

    #[test]
    #[should_panic(expected = "already borrowed")]
    fn test_mutating_while_borrowed_panics_instead_of_uaf() {
        let obj = glib::Object::new::<glib::Object>();
        obj.set_safe_data("active", String::from("live borrow"));

        let borrowed = obj.get_safe_data::<String>("active").unwrap();
        // Attempting to overwrite the same key while `borrowed` is held must panic safely
        obj.set_safe_data("active", String::from("dangling"));
        assert_eq!(borrowed.as_str(), "unreachable");
    }

    #[test]
    fn test_reentrant_access_during_drop_of_replaced_value_succeeds() {
        struct ReentrantOnDrop {
            target: glib::Object,
        }

        impl Drop for ReentrantOnDrop {
            fn drop(&mut self) {
                // Reading the newly installed value while the previous value drops must not deadlock/panic
                let current = self.target.get_safe_data::<u32>("reentrant");
                assert_eq!(current.map(|v| *v), Some(7));
            }
        }

        let obj = glib::Object::new::<glib::Object>();
        obj.set_safe_data(
            "reentrant",
            ReentrantOnDrop {
                target: obj.clone(),
            },
        );
        obj.set_safe_data("reentrant", 7_u32);
        assert_eq!(*obj.get_safe_data::<u32>("reentrant").unwrap(), 7);
    }

    #[test]
    fn test_cross_thread_access_of_non_send_payload_panics_safely() {
        let cancellable = gio::Cancellable::new();
        cancellable.set_safe_data("non_send", Rc::new(123_usize));

        let moved = cancellable.clone();
        let join_res = std::thread::spawn(move || {
            let _ = moved.get_safe_data::<Rc<usize>>("non_send");
        })
        .join();

        assert!(
            join_res.is_err(),
            "cross-thread access to !Send qdata must panic via ThreadGuard"
        );
        // Original thread still reads cleanly
        assert_eq!(
            **cancellable.get_safe_data::<Rc<usize>>("non_send").unwrap(),
            123
        );
    }
}
