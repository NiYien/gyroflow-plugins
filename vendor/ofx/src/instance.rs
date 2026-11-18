use result::*;
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashSet;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::sync::{Mutex, MutexGuard};

thread_local! {
    static BORROWED: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
}

pub(crate) fn is_borrowed(effect: usize) -> bool {
    BORROWED.with(|handles| handles.borrow().contains(&effect))
}

pub(crate) struct InstanceDataCell {
    value: Mutex<Box<dyn Any + Send>>,
}

impl InstanceDataCell {
    pub(crate) fn new<T: Any + Send>(value: T) -> Self {
        Self { value: Mutex::new(Box::new(value)) }
    }

    pub(crate) fn lock<T: Any + Send>(&self, effect: usize) -> Result<InstanceDataGuard<'_, T>> {
        if is_borrowed(effect) { return Err(Error::InvalidAction); }
        let mut guard = self.value.lock().map_err(|_| Error::InvalidValue)?;
        let value = guard.downcast_mut::<T>().ok_or(Error::InvalidValue)? as *mut T;
        BORROWED.with(|handles| { handles.borrow_mut().insert(effect); });
        Ok(InstanceDataGuard { guard, value, effect, _type: PhantomData })
    }
}

pub struct InstanceDataGuard<'a, T> {
    guard: MutexGuard<'a, Box<dyn Any + Send>>,
    value: *mut T,
    effect: usize,
    _type: PhantomData<T>,
}

impl<T> Deref for InstanceDataGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // The checked downcast points into the box retained by this mutex guard.
        unsafe { &*self.value }
    }
}

impl<T> DerefMut for InstanceDataGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T { unsafe { &mut *self.value } }
}

impl<T> Drop for InstanceDataGuard<'_, T> {
    fn drop(&mut self) {
        BORROWED.with(|handles| { handles.borrow_mut().remove(&self.effect); });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn instance_state_serializes_two_threads() {
        let cell = Arc::new(InstanceDataCell::new(0usize));
        let start = Arc::new(Barrier::new(3));
        let workers: Vec<_> = (0..2).map(|_| {
            let cell = cell.clone();
            let start = start.clone();
            thread::spawn(move || {
                start.wait();
                for _ in 0..2000 {
                    let mut state = cell.lock::<usize>(1).unwrap();
                    let before = *state;
                    thread::yield_now();
                    *state = before + 1;
                }
            })
        }).collect();
        start.wait();
        for worker in workers { worker.join().unwrap(); }
        assert_eq!(*cell.lock::<usize>(1).unwrap(), 4000);
    }

    #[test]
    fn recursive_borrow_is_rejected_without_poisoning_the_instance() {
        let cell = InstanceDataCell::new(7usize);
        let state = cell.lock::<usize>(2).unwrap();
        assert!(is_borrowed(2));
        assert!(matches!(cell.lock::<usize>(2), Err(Error::InvalidAction)));
        drop(state);
        assert!(!is_borrowed(2));
        assert_eq!(*cell.lock::<usize>(2).unwrap(), 7);
        assert!(matches!(cell.lock::<String>(2), Err(Error::InvalidValue)));
        assert_eq!(*cell.lock::<usize>(2).unwrap(), 7);
    }
}
