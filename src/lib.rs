use std::{
    borrow::{Borrow, BorrowMut},
    cell::{Ref, RefCell, RefMut},
    cmp::Ordering,
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    ops::{Deref, DerefMut},
    ptr,
    rc::Rc,
    sync::Arc,
};

use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};

pub type Mrc<T, E = ByRef> = Shared<T, ShareUnsync, E>;
pub type Marc<T, E = ByRef> = Shared<T, ShareSync, E>;

pub trait EqualityKind<T> {
    fn eq<S: ShareKind>(a: &S::Inner<T>, b: &S::Inner<T>) -> bool;
}
pub trait CompareKind<T>: EqualityKind<T> {
    fn cmp<S: ShareKind>(a: &S::Inner<T>, b: &S::Inner<T>) -> Ordering;
}
pub trait HashKind<T> {
    fn hash<S: ShareKind, H: Hasher>(inner: &S::Inner<T>, state: &mut H);
}

pub struct ByRef;
pub struct ByVal;

impl<T> EqualityKind<T> for ByRef {
    fn eq<S: ShareKind>(a: &S::Inner<T>, b: &S::Inner<T>) -> bool {
        S::ptr_eq(a, b)
    }
}
impl<T> CompareKind<T> for ByRef {
    fn cmp<S: ShareKind>(a: &S::Inner<T>, b: &S::Inner<T>) -> Ordering {
        S::ptr_cmp(a, b)
    }
}
impl<T> HashKind<T> for ByRef {
    fn hash<S: ShareKind, H: Hasher>(inner: &S::Inner<T>, state: &mut H) {
        S::ptr_hash(inner, state);
    }
}

impl<T: Eq> EqualityKind<T> for ByVal {
    fn eq<S: ShareKind>(a: &S::Inner<T>, b: &S::Inner<T>) -> bool {
        *S::read(a) == *S::read(b)
    }
}
impl<T: Ord> CompareKind<T> for ByVal {
    fn cmp<S: ShareKind>(a: &S::Inner<T>, b: &S::Inner<T>) -> Ordering {
        S::read(a).cmp(&S::read(b))
    }
}
impl<T: Hash> HashKind<T> for ByVal {
    fn hash<S: ShareKind, H: Hasher>(inner: &S::Inner<T>, state: &mut H) {
        S::read(inner).hash(state);
    }
}

pub trait ShareKind {
    type Inner<T>: Clone;
    type ReadGuard<'a, T: 'a>: Deref<Target = T>;
    type WriteGuard<'a, T: 'a>: DerefMut<Target = T>;
    fn make<T>(t: T) -> Self::Inner<T>;
    fn read<T>(inner: &Self::Inner<T>) -> Self::ReadGuard<'_, T>;
    fn write<T>(inner: &mut Self::Inner<T>) -> Self::WriteGuard<'_, T>;
    fn try_read<T>(inner: &Self::Inner<T>) -> Option<Self::ReadGuard<'_, T>>;
    fn try_write<T>(inner: &mut Self::Inner<T>) -> Option<Self::WriteGuard<'_, T>>;
    fn make_mut<T: Clone>(inner: &mut Self::Inner<T>) -> &mut T;
    fn ptr_eq<T>(a: &Self::Inner<T>, b: &Self::Inner<T>) -> bool;
    fn ptr_cmp<T>(a: &Self::Inner<T>, b: &Self::Inner<T>) -> Ordering;
    fn ptr_hash<T, H: Hasher>(inner: &Self::Inner<T>, state: &mut H);
}

pub struct ShareUnsync;

impl ShareKind for ShareUnsync {
    type Inner<T> = Rc<RefCell<T>>;
    type ReadGuard<'a, T: 'a> = Ref<'a, T>;
    type WriteGuard<'a, T: 'a> = RefMut<'a, T>;
    fn make<T>(t: T) -> Self::Inner<T> {
        Rc::new(RefCell::new(t))
    }
    fn read<T>(inner: &Self::Inner<T>) -> Self::ReadGuard<'_, T> {
        RefCell::borrow(inner)
    }
    fn write<T>(inner: &mut Self::Inner<T>) -> Self::WriteGuard<'_, T> {
        RefCell::borrow_mut(inner)
    }
    fn try_read<T>(inner: &Self::Inner<T>) -> Option<Self::ReadGuard<'_, T>> {
        inner.try_borrow().ok()
    }
    fn try_write<T>(inner: &mut Self::Inner<T>) -> Option<Self::WriteGuard<'_, T>> {
        inner.try_borrow_mut().ok()
    }
    fn make_mut<T: Clone>(inner: &mut Self::Inner<T>) -> &mut T {
        Rc::make_mut(inner).get_mut()
    }
    fn ptr_eq<T>(a: &Self::Inner<T>, b: &Self::Inner<T>) -> bool {
        Rc::ptr_eq(a, b)
    }
    fn ptr_cmp<T>(a: &Self::Inner<T>, b: &Self::Inner<T>) -> Ordering {
        Rc::as_ptr(a).cmp(&Rc::as_ptr(b))
    }
    fn ptr_hash<T, H: Hasher>(inner: &Self::Inner<T>, state: &mut H) {
        ptr::hash(Rc::as_ptr(inner), state);
    }
}

pub struct ShareSync;

impl ShareKind for ShareSync {
    type Inner<T> = Arc<RwLock<T>>;
    type ReadGuard<'a, T: 'a> = RwLockReadGuard<'a, T>;
    type WriteGuard<'a, T: 'a> = RwLockWriteGuard<'a, T>;
    fn make<T>(t: T) -> Self::Inner<T> {
        Arc::new(RwLock::new(t))
    }
    fn read<T>(inner: &Self::Inner<T>) -> Self::ReadGuard<'_, T> {
        inner.read()
    }
    fn write<T>(inner: &mut Self::Inner<T>) -> Self::WriteGuard<'_, T> {
        inner.write()
    }
    fn try_read<T>(inner: &Self::Inner<T>) -> Option<Self::ReadGuard<'_, T>> {
        inner.try_read()
    }
    fn try_write<T>(inner: &mut Self::Inner<T>) -> Option<Self::WriteGuard<'_, T>> {
        inner.try_write()
    }
    fn make_mut<T: Clone>(inner: &mut Self::Inner<T>) -> &mut T {
        if Arc::get_mut(inner).is_none() {
            let value = inner.read().clone();
            *inner = Arc::new(RwLock::new(value));
        }
        Arc::get_mut(inner)
            .expect("the Arc was just created")
            .get_mut()
    }
    fn ptr_eq<T>(a: &Self::Inner<T>, b: &Self::Inner<T>) -> bool {
        Arc::ptr_eq(a, b)
    }
    fn ptr_cmp<T>(a: &Self::Inner<T>, b: &Self::Inner<T>) -> Ordering {
        Arc::as_ptr(a).cmp(&Arc::as_ptr(b))
    }
    fn ptr_hash<T, H: Hasher>(inner: &Self::Inner<T>, state: &mut H) {
        ptr::hash(Arc::as_ptr(inner), state);
    }
}

pub struct Shared<T, S: ShareKind, E = ByRef>(S::Inner<T>, PhantomData<E>);

impl<T, S: ShareKind, E> From<T> for Shared<T, S, E> {
    fn from(t: T) -> Self {
        Shared::new(t)
    }
}

impl<T, S: ShareKind> Shared<T, S, ByRef> {
    pub fn new_by_ref(t: T) -> Self {
        Shared::new(t)
    }
}

impl<T, S: ShareKind> Shared<T, S, ByVal> {
    pub fn new_by_val(t: T) -> Self {
        Shared::new(t)
    }
}

impl<T, S: ShareKind, E> Shared<T, S, E> {
    pub fn new(t: T) -> Self {
        Shared(S::make(t), PhantomData)
    }
    pub fn get(&self) -> ReadGuard<T, S> {
        ReadGuard(S::read(&self.0))
    }
    pub fn get_mut(&mut self) -> WriteGuard<T, S> {
        WriteGuard(S::write(&mut self.0))
    }
    pub fn try_get(&self) -> Option<ReadGuard<T, S>> {
        S::try_read(&self.0).map(ReadGuard)
    }
    pub fn try_get_mut(&mut self) -> Option<WriteGuard<T, S>> {
        S::try_write(&mut self.0).map(WriteGuard)
    }
    pub fn set(&mut self, t: T) {
        *self.get_mut() = t;
    }
    pub fn try_set(&mut self, t: T) -> bool {
        if let Some(mut guard) = self.try_get_mut() {
            *guard = t;
            true
        } else {
            false
        }
    }
    pub fn make_mut(&mut self) -> &mut T
    where
        T: Clone,
    {
        S::make_mut(&mut self.0)
    }
    pub fn copied(&self) -> T
    where
        T: Copy,
    {
        *self.get()
    }
    pub fn cloned(&self) -> T
    where
        T: Clone,
    {
        self.get().clone()
    }
    pub fn bind<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        f(&self.get())
    }
}

impl<T, S: ShareKind, E> Clone for Shared<T, S, E> {
    fn clone(&self) -> Self {
        Shared(S::Inner::<T>::clone(&self.0), PhantomData)
    }
}
impl<T, S: ShareKind, E> fmt::Debug for Shared<T, S, E>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(guard) = self.try_get() {
            write!(f, "{:?}", *guard)
        } else {
            write!(f, "<locked>")
        }
    }
}

impl<T, S: ShareKind, E> fmt::Display for Shared<T, S, E>
where
    T: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(guard) = self.try_get() {
            write!(f, "{}", *guard)
        } else {
            write!(f, "<locked>")
        }
    }
}

impl<T, S: ShareKind, E: EqualityKind<T>> PartialEq for Shared<T, S, E> {
    fn eq(&self, other: &Self) -> bool {
        E::eq::<S>(&self.0, &other.0)
    }
}

impl<T: PartialEq, S: ShareKind> PartialEq<T> for Shared<T, S, ByVal> {
    fn eq(&self, other: &T) -> bool {
        *self.get() == *other
    }
}

impl<S: ShareKind> PartialEq<str> for Shared<String, S, ByVal> {
    fn eq(&self, other: &str) -> bool {
        *self.get() == other
    }
}

impl<'a, S: ShareKind> PartialEq<&'a str> for Shared<String, S, ByVal> {
    fn eq(&self, other: &&'a str) -> bool {
        *self.get() == *other
    }
}

impl<T, S: ShareKind, E: EqualityKind<T>> Eq for Shared<T, S, E> {}

impl<T, S: ShareKind, E: CompareKind<T>> PartialOrd for Shared<T, S, E> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(E::cmp::<S>(&self.0, &other.0))
    }
}

impl<T: PartialOrd, S: ShareKind> PartialOrd<T> for Shared<T, S, ByVal> {
    fn partial_cmp(&self, other: &T) -> Option<Ordering> {
        Some((*self.get()).partial_cmp(other).unwrap())
    }
}

impl<T, S: ShareKind, E: CompareKind<T>> Ord for Shared<T, S, E> {
    fn cmp(&self, other: &Self) -> Ordering {
        E::cmp::<S>(&self.0, &other.0)
    }
}

impl<T, S: ShareKind, E: HashKind<T>> Hash for Shared<T, S, E> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        E::hash::<S, _>(&self.0, state)
    }
}

pub struct ReadGuard<'a, T: 'a, K: ShareKind>(K::ReadGuard<'a, T>);
pub struct WriteGuard<'a, T: 'a, K: ShareKind>(K::WriteGuard<'a, T>);

macro_rules! guard_impl {
    ($ty:ident) => {
        impl<'a, T, K: ShareKind> Deref for $ty<'a, T, K> {
            type Target = T;
            fn deref(&self) -> &T {
                self.0.deref()
            }
        }

        impl<'a, T: fmt::Debug, K: ShareKind> fmt::Debug for $ty<'a, T, K> {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                self.deref().fmt(f)
            }
        }

        impl<'a, T: fmt::Display, K: ShareKind> fmt::Display for $ty<'a, T, K> {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                self.deref().fmt(f)
            }
        }

        impl<'a, T: PartialEq, K: ShareKind> PartialEq for $ty<'a, T, K> {
            fn eq(&self, other: &Self) -> bool {
                self.deref().eq(other.deref())
            }
        }

        impl<'a, T: PartialEq, K: ShareKind> PartialEq<T> for $ty<'a, T, K> {
            fn eq(&self, other: &T) -> bool {
                self.deref().eq(other)
            }
        }

        impl<'a, K: ShareKind> PartialEq<str> for $ty<'a, String, K> {
            fn eq(&self, other: &str) -> bool {
                self.deref().eq(other)
            }
        }

        impl<'a, 'b, K: ShareKind> PartialEq<&'b str> for $ty<'a, String, K> {
            fn eq(&self, other: &&'b str) -> bool {
                self.deref().eq(other)
            }
        }

        impl<'a, T: Eq, K: ShareKind> Eq for $ty<'a, T, K> {}

        impl<'a, T: PartialOrd, K: ShareKind> PartialOrd for $ty<'a, T, K> {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                self.deref().partial_cmp(other.deref())
            }
        }

        impl<'a, T: Ord, K: ShareKind> Ord for $ty<'a, T, K> {
            fn cmp(&self, other: &Self) -> Ordering {
                self.deref().cmp(other.deref())
            }
        }

        impl<'a, T: Hash, K: ShareKind> Hash for $ty<'a, T, K> {
            fn hash<H: Hasher>(&self, state: &mut H) {
                self.deref().hash(state)
            }
        }

        impl<'a, T, K: ShareKind> AsRef<T> for $ty<'a, T, K> {
            fn as_ref(&self) -> &T {
                self.deref()
            }
        }

        impl<'a, T, K: ShareKind> Borrow<T> for $ty<'a, T, K> {
            fn borrow(&self) -> &T {
                self.deref()
            }
        }
    };
}

guard_impl!(ReadGuard);
guard_impl!(WriteGuard);

impl<'a, T, K: ShareKind> DerefMut for WriteGuard<'a, T, K> {
    fn deref_mut(&mut self) -> &mut T {
        self.0.deref_mut()
    }
}

impl<'a, T, K: ShareKind> AsMut<T> for WriteGuard<'a, T, K> {
    fn as_mut(&mut self) -> &mut T {
        self.deref_mut()
    }
}

impl<'a, T, K: ShareKind> BorrowMut<T> for WriteGuard<'a, T, K> {
    fn borrow_mut(&mut self) -> &mut T {
        self.deref_mut()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn equality() {
        let a = Mrc::new_by_ref(1);
        let b = Mrc::new_by_ref(1);
        assert_ne!(a, b);

        let a = Mrc::new_by_val(1);
        let b = Mrc::new_by_val(1);
        assert_eq!(a, b);
    }
}
