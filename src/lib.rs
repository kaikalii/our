#![warn(missing_docs)]

/*!
# Description

`our` provides a highly generic shared mutable data abstraction.

# Usage

[`Shared`] is a generic wrapper around what is usually a smart pointer to something with interior mutability.
It provides a way to construct and access shared mutable data, and also provides a way to compare and hash shared values.

Even though [`Shared`] is usually implemented with some kind of interior mutability,
[`Shared`]'s methods that return write guards to the shared value require mutable references to the [`Shared`] itself.
While this can be easily circumvented by simply cloning the [`Shared`], it is implemented this way to try
to prevent at compile-time accidentally trying to acquire two exclusive guards, which would panic for non-thread-safe
[`Shared`]s and deadlock for thread-safe ones.

[`Shared`] has three type parameters:
- The type of the shared value
- A [`ShareKind`], which determines how the shared value is constructed and accessed
    - [`ShareUnsync`] is a non-thread-safe shared value implemented as `Rc<RefCell<T>>`
    - [`ShareSync`] is a thread-safe shared value implemented as `Arc<parking_lot::RwLock<T>>`
- A type which usually implements [`EqualityKind`], [`CompareKind`], and [`HashKind`],
which determines how shared values are compared and hashed.
    - [`ByRef`] compares and hashes by reference
    - [`ByVal`] compares and hashes by value

# Type aliases

There are four type aliases for [`Shared`] provided for convenience:

|                    |Non-thread-safe|Thread-safe  |
|--------------------|---------------|-------------|
|Compare by reference|[`UnsyncByRef`]|[`SyncByRef`]|
|Compare by value    |[`UnsyncByVal`]|[`SyncByVal`]|

# Example
```
use our::*;

// `SyncByRef` is a thread-safe shared value with by-reference comparison and hashing.
let mut a = SyncByRef::new(0);
let mut b = a.clone();
std::thread::spawn(move || b.set(1)).join().unwrap();
assert_eq!(a.get(), 1);
let c = SyncByRef::new(1);
assert_ne!(a, c); // Notice that while while `a` and `c` both equal `1`,
                  // they do not compare equal because they are different
                  // pointers.

// `UnsyncByVal` is a non-thread-safe shared value with by-value comparison and hashing.
let a = UnsyncByVal::new(5);
let b = UnsyncByVal::new(5);
assert_eq!(a, b); // Notice that `a` and `b` compare equal
                  // even though they are different pointers.
```
*/

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

use parking_lot::{
    MappedRwLockReadGuard, MappedRwLockWriteGuard, RwLock, RwLockReadGuard, RwLockWriteGuard,
};

/// A non-thread-safe shared value with by-reference comparison and hashing
pub type UnsyncByRef<T> = Shared<T, ShareUnsync, ByRef>;
/// A thread-safe shared value with by-reference comparison and hashing
pub type SyncByRef<T> = Shared<T, ShareSync, ByRef>;
/// A non-thread-safe shared value with by-value comparison and hashing
pub type UnsyncByVal<T> = Shared<T, ShareUnsync, ByVal>;
/// A thread-safe shared value with by-value comparison and hashing
pub type SyncByVal<T> = Shared<T, ShareSync, ByVal>;

/// A way of comparing two shared value for equality
pub trait EqualityKind<T> {
    /// Compare two shared values for equality
    fn eq<S: ShareKind>(a: &S::Inner<T>, b: &S::Inner<T>) -> bool;
}
/// A way of comparing two shared value for ordering
pub trait CompareKind<T>: EqualityKind<T> {
    /// Compare two shared values for ordering
    fn cmp<S: ShareKind>(a: &S::Inner<T>, b: &S::Inner<T>) -> Ordering;
}
/// A way of hashing a shared value
pub trait HashKind<T> {
    /// Hash a shared value
    fn hash<S: ShareKind, H: Hasher>(inner: &S::Inner<T>, state: &mut H);
}

/// Compare and hash shared values by reference
pub struct ByRef;
/// Compare and hash shared values by value
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

/// A family of guards
pub trait GuardKind {
    /// The type of the read-only guard
    type ReadGuard<'a, T: 'a>: Deref<Target = T>;
    /// The type of the read-write guard
    type WriteGuard<'a, T: 'a>: DerefMut<Target = T>;
    /// Map a read guard to guard an inner value
    fn map_read<T, U, F: FnOnce(&T) -> &U>(
        guard: Self::ReadGuard<'_, T>,
        f: F,
    ) -> Self::ReadGuard<'_, U>;
    /// Map a write guard to guard an inner value
    fn map_write<T, U, F: FnOnce(&mut T) -> &mut U>(
        guard: Self::WriteGuard<'_, T>,
        f: F,
    ) -> Self::WriteGuard<'_, U>;
    /// Filter map a read guard to guard an inner value
    fn filter_map_read<T, U, F: FnOnce(&T) -> Option<&U>>(
        guard: Self::ReadGuard<'_, T>,
        f: F,
    ) -> Result<Self::ReadGuard<'_, U>, Self::ReadGuard<'_, T>>;
    /// Filter map a write guard to guard an inner value
    fn filter_map_write<T, U, F: FnOnce(&mut T) -> Option<&mut U>>(
        guard: Self::WriteGuard<'_, T>,
        f: F,
    ) -> Result<Self::WriteGuard<'_, U>, Self::WriteGuard<'_, T>>;
}

/// A non-thread-safe [`GuardKind`]
pub struct GuardUnsync;
/// A thread-safe guard [`GuardKind`]
pub struct GuardSync;

impl GuardKind for GuardUnsync {
    type ReadGuard<'a, T: 'a> = Ref<'a, T>;
    type WriteGuard<'a, T: 'a> = RefMut<'a, T>;
    fn map_read<T, U, F: FnOnce(&T) -> &U>(
        guard: Self::ReadGuard<'_, T>,
        f: F,
    ) -> Self::ReadGuard<'_, U> {
        Ref::map(guard, f)
    }
    fn map_write<T, U, F: FnOnce(&mut T) -> &mut U>(
        guard: Self::WriteGuard<'_, T>,
        f: F,
    ) -> Self::WriteGuard<'_, U> {
        RefMut::map(guard, f)
    }
    fn filter_map_read<T, U, F: FnOnce(&T) -> Option<&U>>(
        guard: Self::ReadGuard<'_, T>,
        f: F,
    ) -> Result<Self::ReadGuard<'_, U>, Self::ReadGuard<'_, T>> {
        Ref::filter_map(guard, f)
    }
    fn filter_map_write<T, U, F: FnOnce(&mut T) -> Option<&mut U>>(
        guard: Self::WriteGuard<'_, T>,
        f: F,
    ) -> Result<Self::WriteGuard<'_, U>, Self::WriteGuard<'_, T>> {
        RefMut::filter_map(guard, f)
    }
}

impl GuardKind for GuardSync {
    type ReadGuard<'a, T: 'a> = MappedRwLockReadGuard<'a, T>;
    type WriteGuard<'a, T: 'a> = MappedRwLockWriteGuard<'a, T>;
    fn map_read<T, U, F: FnOnce(&T) -> &U>(
        guard: Self::ReadGuard<'_, T>,
        f: F,
    ) -> Self::ReadGuard<'_, U> {
        MappedRwLockReadGuard::map(guard, f)
    }
    fn map_write<T, U, F: FnOnce(&mut T) -> &mut U>(
        guard: Self::WriteGuard<'_, T>,
        f: F,
    ) -> Self::WriteGuard<'_, U> {
        MappedRwLockWriteGuard::map(guard, f)
    }
    fn filter_map_read<T, U, F: FnOnce(&T) -> Option<&U>>(
        guard: Self::ReadGuard<'_, T>,
        f: F,
    ) -> Result<Self::ReadGuard<'_, U>, Self::ReadGuard<'_, T>> {
        MappedRwLockReadGuard::try_map(guard, f)
    }
    fn filter_map_write<T, U, F: FnOnce(&mut T) -> Option<&mut U>>(
        guard: Self::WriteGuard<'_, T>,
        f: F,
    ) -> Result<Self::WriteGuard<'_, U>, Self::WriteGuard<'_, T>> {
        MappedRwLockWriteGuard::try_map(guard, f)
    }
}

/// A way of constructing and accessing shared mutable state
pub trait ShareKind {
    /// The inner wrapper, usually a smart pointer wrapping something with interior mutability
    type Inner<T>: Clone;
    /// The guard kind
    type GuardKind: GuardKind;
    /// Make a new inner value
    fn make<T>(t: T) -> Self::Inner<T>;
    /// Get a read-only guard to the value
    fn read<T>(inner: &Self::Inner<T>) -> <Self::GuardKind as GuardKind>::ReadGuard<'_, T>;
    /// Get a read-write guard to the value
    fn write<T>(inner: &mut Self::Inner<T>) -> <Self::GuardKind as GuardKind>::WriteGuard<'_, T>;
    /// Try to get a read-only guard to the value
    fn try_read<T>(
        inner: &Self::Inner<T>,
    ) -> Option<<Self::GuardKind as GuardKind>::ReadGuard<'_, T>>;
    /// Try to get a read-write guard to the value
    fn try_write<T>(
        inner: &mut Self::Inner<T>,
    ) -> Option<<Self::GuardKind as GuardKind>::WriteGuard<'_, T>>;
    /// Try to get a mutable reference to the value
    ///
    /// Should return `None` if clones exist or a guard is held
    fn as_mut<T>(inner: &mut Self::Inner<T>) -> Option<&mut T>;
    /// Get a mutable reference to the value, cloning it if necessary
    fn make_mut<T: Clone>(inner: &mut Self::Inner<T>) -> &mut T {
        if Self::as_mut(inner).is_none() {
            let value = Self::read(inner).clone();
            *inner = Self::make(value);
        }
        Self::as_mut(inner).unwrap()
    }
    /// Compare two inner values for pointer equality
    fn ptr_eq<T>(a: &Self::Inner<T>, b: &Self::Inner<T>) -> bool;
    /// Compare two inner values for pointer ordering
    fn ptr_cmp<T>(a: &Self::Inner<T>, b: &Self::Inner<T>) -> Ordering;
    /// Hash an inner value by pointer
    fn ptr_hash<T, H: Hasher>(inner: &Self::Inner<T>, state: &mut H);
}

/// A non-thread-safe [`ShareKind`]
pub struct ShareUnsync;

impl ShareKind for ShareUnsync {
    type Inner<T> = Rc<RefCell<T>>;
    type GuardKind = GuardUnsync;
    fn make<T>(t: T) -> Self::Inner<T> {
        Rc::new(RefCell::new(t))
    }
    fn read<T>(inner: &Self::Inner<T>) -> <Self::GuardKind as GuardKind>::ReadGuard<'_, T> {
        RefCell::borrow(inner)
    }
    fn write<T>(inner: &mut Self::Inner<T>) -> <Self::GuardKind as GuardKind>::WriteGuard<'_, T> {
        RefCell::borrow_mut(inner)
    }
    fn try_read<T>(
        inner: &Self::Inner<T>,
    ) -> Option<<Self::GuardKind as GuardKind>::ReadGuard<'_, T>> {
        inner.try_borrow().ok()
    }
    fn try_write<T>(
        inner: &mut Self::Inner<T>,
    ) -> Option<<Self::GuardKind as GuardKind>::WriteGuard<'_, T>> {
        inner.try_borrow_mut().ok()
    }
    fn as_mut<T>(inner: &mut Self::Inner<T>) -> Option<&mut T> {
        Rc::get_mut(inner).map(RefCell::get_mut)
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

/// A thread-safe [`ShareKind`]
pub struct ShareSync;

impl ShareKind for ShareSync {
    type Inner<T> = Arc<RwLock<T>>;
    type GuardKind = GuardSync;
    fn make<T>(t: T) -> Self::Inner<T> {
        Arc::new(RwLock::new(t))
    }
    fn read<T>(inner: &Self::Inner<T>) -> <Self::GuardKind as GuardKind>::ReadGuard<'_, T> {
        RwLockReadGuard::map(inner.read(), |x| x)
    }
    fn write<T>(inner: &mut Self::Inner<T>) -> <Self::GuardKind as GuardKind>::WriteGuard<'_, T> {
        RwLockWriteGuard::map(inner.write(), |x| x)
    }
    fn try_read<T>(
        inner: &Self::Inner<T>,
    ) -> Option<<Self::GuardKind as GuardKind>::ReadGuard<'_, T>> {
        inner.try_read().map(|x| RwLockReadGuard::map(x, |x| x))
    }
    fn try_write<T>(
        inner: &mut Self::Inner<T>,
    ) -> Option<<Self::GuardKind as GuardKind>::WriteGuard<'_, T>> {
        inner.try_write().map(|x| RwLockWriteGuard::map(x, |x| x))
    }
    fn as_mut<T>(inner: &mut Self::Inner<T>) -> Option<&mut T> {
        Arc::get_mut(inner).map(RwLock::get_mut)
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

/// A shared, mutable value
///
/// See also the [type aliases](index.html#type-aliases).
///
/// The implementation can be chosen with the `S` [`ShareKind`] type parameter.
/// This crate provides [`ShareUnsync`] for non-thread-safe sharing
/// and [`ShareSync`] for thread-safe sharing.
///
/// The `E` type parameter determines how the value is compared and hashed.
/// This crate provides [`ByRef`] for comparing and hashing by reference
/// and [`ByVal`] for comparing and hashing by value.
pub struct Shared<T, S: ShareKind, E = ByRef>(S::Inner<T>, PhantomData<E>);

impl<T: Default, S: ShareKind, E> Default for Shared<T, S, E> {
    fn default() -> Self {
        Shared::new(Default::default())
    }
}

impl<T, S: ShareKind, E> From<T> for Shared<T, S, E> {
    fn from(t: T) -> Self {
        Shared::new(t)
    }
}

impl<T, S: ShareKind> Shared<T, S, ByRef> {
    /// Create a new shared value that compares and hashes by reference
    pub fn new_by_ref(t: T) -> Self {
        Shared::new(t)
    }
}

impl<T, S: ShareKind> Shared<T, S, ByVal> {
    /// Create a new shared value that compares and hashes by value
    pub fn new_by_val(t: T) -> Self {
        Shared::new(t)
    }
}

impl<T, S: ShareKind, E> Shared<T, S, E> {
    /// Create a new shared value
    pub fn new(t: T) -> Self {
        Shared(S::make(t), PhantomData)
    }
    /// Get a read guard to the value
    pub fn get(&self) -> ReadGuard<T, S::GuardKind> {
        ReadGuard(S::read(&self.0))
    }
    /// Get a write guard to the value
    pub fn get_mut(&mut self) -> WriteGuard<T, S::GuardKind> {
        WriteGuard(S::write(&mut self.0))
    }
    /// Try to get a read guard to the value
    ///
    /// Returns `None` if a write guard is currently held
    pub fn try_get(&self) -> Option<ReadGuard<T, S::GuardKind>> {
        S::try_read(&self.0).map(ReadGuard)
    }
    /// Try to get a write guard to the value
    ///
    /// Returns `None` if a read or write guard is currently held
    pub fn try_get_mut(&mut self) -> Option<WriteGuard<T, S::GuardKind>> {
        S::try_write(&mut self.0).map(WriteGuard)
    }
    /// Set the value
    pub fn set(&mut self, t: T) {
        *self.get_mut() = t;
    }
    /// Try to set the value
    ///
    /// Fails if a read or write guard is currently held
    ///
    /// Returns whether the value was set
    pub fn try_set(&mut self, t: T) -> bool {
        if let Some(mut guard) = self.try_get_mut() {
            *guard = t;
            true
        } else {
            false
        }
    }
    /// Get a mutable reference to the value, cloning if a guard is held
    pub fn make_mut(&mut self) -> &mut T
    where
        T: Clone,
    {
        S::make_mut(&mut self.0)
    }
    /// Copy out the value
    pub fn copied(&self) -> T
    where
        T: Copy,
    {
        *self.get()
    }
    /// Clone out the value
    pub fn cloned(&self) -> T
    where
        T: Clone,
    {
        self.get().clone()
    }
    /// Try to get a mutable reference to the value
    ///
    /// Returns `None` if clones exist or a guard is held
    pub fn as_mut(&mut self) -> Option<&mut T> {
        S::as_mut(&mut self.0)
    }
    /// Get a read guard and apply a function to the value
    ///
    /// Useful for one-liners
    pub fn bind<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        f(&self.get())
    }
    /// Get a write guard and apply a function to the value
    ///
    /// Useful for one-liners
    ///
    /// This function does not acquire a lock if there are no
    /// clones or guards
    pub fn bind_mut<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R,
    {
        if let Some(value) = self.as_mut() {
            f(value)
        } else {
            f(&mut self.get_mut())
        }
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

impl<'a, T, S: ShareKind, E: EqualityKind<T>> PartialEq<&'a Self> for Shared<T, S, E> {
    fn eq(&self, other: &&'a Self) -> bool {
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

/// A guard that allows read-only access to a shared value
///
/// Guards in this crate always compare and hash by value
pub struct ReadGuard<'a, T: 'a, G: GuardKind>(G::ReadGuard<'a, T>);
/// A guard that allows read-write access to a shared value
///
/// Guards in this crate always compare and hash by value
pub struct WriteGuard<'a, T: 'a, G: GuardKind>(G::WriteGuard<'a, T>);

macro_rules! guard_impl {
    ($ty:ident) => {
        impl<'a, T, G: GuardKind> Deref for $ty<'a, T, G> {
            type Target = T;
            fn deref(&self) -> &T {
                self.0.deref()
            }
        }

        impl<'a, T: fmt::Debug, G: GuardKind> fmt::Debug for $ty<'a, T, G> {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                self.deref().fmt(f)
            }
        }

        impl<'a, T: fmt::Display, G: GuardKind> fmt::Display for $ty<'a, T, G> {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                self.deref().fmt(f)
            }
        }

        impl<'a, T: PartialEq, G: GuardKind> PartialEq for $ty<'a, T, G> {
            fn eq(&self, other: &Self) -> bool {
                self.deref().eq(other.deref())
            }
        }

        impl<'a, 'b, T: PartialEq, G: GuardKind> PartialEq<&'b Self> for $ty<'a, T, G> {
            fn eq(&self, other: &&'b Self) -> bool {
                self.deref().eq(other.deref())
            }
        }

        impl<'a, T: PartialEq, G: GuardKind> PartialEq<T> for $ty<'a, T, G> {
            fn eq(&self, other: &T) -> bool {
                self.deref().eq(other)
            }
        }

        impl<'a, G: GuardKind> PartialEq<str> for $ty<'a, String, G> {
            fn eq(&self, other: &str) -> bool {
                self.deref().eq(other)
            }
        }

        impl<'a, 'b, G: GuardKind> PartialEq<&'b str> for $ty<'a, String, G> {
            fn eq(&self, other: &&'b str) -> bool {
                self.deref().eq(other)
            }
        }

        impl<'a, T: Eq, G: GuardKind> Eq for $ty<'a, T, G> {}

        impl<'a, T: PartialOrd, G: GuardKind> PartialOrd for $ty<'a, T, G> {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                self.deref().partial_cmp(other.deref())
            }
        }

        impl<'a, T: Ord, G: GuardKind> Ord for $ty<'a, T, G> {
            fn cmp(&self, other: &Self) -> Ordering {
                self.deref().cmp(other.deref())
            }
        }

        impl<'a, T: Hash, G: GuardKind> Hash for $ty<'a, T, G> {
            fn hash<H: Hasher>(&self, state: &mut H) {
                self.deref().hash(state)
            }
        }

        impl<'a, T, G: GuardKind> AsRef<T> for $ty<'a, T, G> {
            fn as_ref(&self) -> &T {
                self.deref()
            }
        }

        impl<'a, T, G: GuardKind> Borrow<T> for $ty<'a, T, G> {
            fn borrow(&self) -> &T {
                self.deref()
            }
        }
    };
}

guard_impl!(ReadGuard);
guard_impl!(WriteGuard);

impl<'a, T, G: GuardKind> DerefMut for WriteGuard<'a, T, G> {
    fn deref_mut(&mut self) -> &mut T {
        self.0.deref_mut()
    }
}

impl<'a, T, G: GuardKind> AsMut<T> for WriteGuard<'a, T, G> {
    fn as_mut(&mut self) -> &mut T {
        self.deref_mut()
    }
}

impl<'a, T, G: GuardKind> BorrowMut<T> for WriteGuard<'a, T, G> {
    fn borrow_mut(&mut self) -> &mut T {
        self.deref_mut()
    }
}

impl<'a, T, G: GuardKind> ReadGuard<'a, T, G> {
    /// Maps this guard to an inner value
    pub fn map<U, F: FnOnce(&T) -> &U>(self, f: F) -> ReadGuard<'a, U, G> {
        ReadGuard(G::map_read(self.0, f))
    }
    /// Maps this guard to an inner value, or returns the original guard
    /// if the mapping function returns `None`
    pub fn filter_map<U, F: FnOnce(&T) -> Option<&U>>(
        self,
        f: F,
    ) -> Result<ReadGuard<'a, U, G>, Self> {
        G::filter_map_read(self.0, f)
            .map(ReadGuard)
            .map_err(ReadGuard)
    }
}

impl<'a, T, G: GuardKind> WriteGuard<'a, T, G> {
    /// Maps this guard to an inner value
    pub fn map<U, F: FnOnce(&mut T) -> &mut U>(self, f: F) -> WriteGuard<'a, U, G> {
        WriteGuard(G::map_write(self.0, f))
    }
    /// Maps this guard to an inner value, or returns the original guard
    /// if the mapping function returns `None`
    pub fn filter_map<U, F: FnOnce(&mut T) -> Option<&mut U>>(
        self,
        f: F,
    ) -> Result<WriteGuard<'a, U, G>, Self> {
        G::filter_map_write(self.0, f)
            .map(WriteGuard)
            .map_err(WriteGuard)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn equality() {
        let a = UnsyncByRef::new(1);
        let b = UnsyncByRef::new(1);
        assert_ne!(a, b);

        let a = UnsyncByVal::new_by_val(1);
        let b = UnsyncByVal::new_by_val(1);
        assert_eq!(a, b);
    }

    #[test]
    fn map() {
        struct Foo {
            s: String,
        }

        let mut foo = UnsyncByRef::new(Foo {
            s: "hello".to_string(),
        });

        let mut s = foo.get_mut().map(|f| &mut f.s);
        *s = "world".to_string();
        drop(s);

        assert_eq!(foo.get().s, "world");
    }

    #[test]
    fn as_mut() {
        let mut x = UnsyncByRef::new(1);
        *x.as_mut().unwrap() += 1;
        assert_eq!(x.get(), 2);

        let _y = x.clone();
        assert!(x.as_mut().is_none());
    }

    #[test]
    fn bind() {
        let mut x = UnsyncByRef::new(1);
        let y = x.bind_mut(|x| {
            *x += 1;
            *x * *x
        });
        assert_eq!(x.get(), 2);
        assert_eq!(y, 4);

        let _x2 = x.clone();
        let y = x.bind_mut(|x| {
            *x += 1;
            *x * *x
        });
        assert_eq!(x.get(), 3);
        assert_eq!(y, 9);
    }
}
