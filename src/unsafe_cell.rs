use std::{
    cell::UnsafeCell,
    ops::{Deref, DerefMut},
};

pub struct RalloUnsafeCell<T: ?Sized> {
    inner: UnsafeCell<T>,
}
unsafe impl<T: ?Sized> Send for RalloUnsafeCell<T> {}
unsafe impl<T: ?Sized> Sync for RalloUnsafeCell<T> {}
impl<T> RalloUnsafeCell<T> {
    pub const fn new(value: T) -> Self {
        RalloUnsafeCell {
            inner: UnsafeCell::new(value),
        }
    }
    pub fn get(&self) -> *mut T {
        self.inner.get()
    }
}
impl<T: ?Sized> Deref for RalloUnsafeCell<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.inner.get() }
    }
}
impl<T: ?Sized> DerefMut for RalloUnsafeCell<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.inner.get() }
    }
}
