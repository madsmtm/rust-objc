use core::borrow;
use core::fmt;
use core::hash;
use core::marker::{PhantomData, Unpin};
use core::mem;
use core::ops::Deref;
use core::ptr::NonNull;

use super::AutoreleasePool;
use super::Owned;
use crate::runtime::{self, Object};

/// An smart pointer that strongly references an object, ensuring it won't be
/// deallocated.
///
/// This doesn't own the object, so it is not safe to obtain a mutable
/// reference from this. For that, see [`Owned`].
///
/// This is guaranteed to have the same size as the underlying pointer.
///
/// TODO: Something about the fact that we haven't made the methods associated
/// for [reasons]???
///
/// ## Caveats
///
/// If the inner type implements [`Drop`], that implementation will not be
/// called, since there is no way to ensure that the Objective-C runtime will
/// do so. If you need to run some code when the object is destroyed,
/// implement the `dealloc` selector instead.
///
/// TODO: Restrict the possible types with some kind of unsafe marker trait?
///
/// TODO: Explain similarities with `Arc` and `RefCell`.
#[repr(transparent)]
pub struct Retained<T> {
    /// A pointer to the contained object.
    ///
    /// It is important that this is `NonNull`, since we want to dereference
    /// it later.
    ///
    /// Usually the contained object would be an [extern type][extern-type-rfc]
    /// (when that gets stabilized), or a type such as:
    /// ```
    /// pub struct MyType {
    ///     _data: [u8; 0], // TODO: `UnsafeCell`?
    /// }
    /// ```
    ///
    /// DSTs that carry metadata cannot be used here, so unsure if we should
    /// have a `?Sized` bound?
    ///
    /// TODO:
    /// https://doc.rust-lang.org/book/ch19-04-advanced-types.html#dynamically-sized-types-and-the-sized-trait
    /// https://doc.rust-lang.org/nomicon/exotic-sizes.html
    /// https://doc.rust-lang.org/core/ptr/trait.Pointee.html
    /// https://doc.rust-lang.org/core/ptr/traitalias.Thin.html
    ///
    /// [extern-type-rfc]: https://github.com/rust-lang/rfcs/blob/master/text/1861-extern-types.md
    ptr: NonNull<T>, // T is immutable, so covariance is correct
    /// TODO:
    /// https://github.com/rust-lang/rfcs/blob/master/text/0769-sound-generic-drop.md#phantom-data
    phantom: PhantomData<T>,
}

/// The `Send` implementation requires `T: Sync` because `Retained` gives
/// access to `&T`.
///
/// Additiontally, it requires `T: Send` because if `T: !Send`, you could
/// clone a `Retained`, send it to another thread, and drop the clone last,
/// making `dealloc` get called on the other thread, violating `T: !Send`.
unsafe impl<T: Sync + Send> Send for Retained<T> {}

/// The `Sync` implementation requires `T: Sync` because `&Retained` gives
/// access to `&T`.
///
/// Additiontally, it requires `T: Send`, because if `T: !Send`, you could
/// clone a `&Retained` from another thread, and drop the clone last, making
/// `dealloc` get called on the other thread, violating `T: !Send`.
unsafe impl<T: Sync + Send> Sync for Retained<T> {}

impl<T> Retained<T> {
    /// Constructs a `Retained<T>` to an object that already has a +1 retain
    /// count. This will not retain the object.
    ///
    /// When dropped, the object will be released.
    ///
    /// This is used when you have a retain count that has been handed off
    /// from somewhere else, usually Objective-C methods with the
    /// `ns_returns_retained` attribute. See [`Owned::new`] for the more
    /// common case when creating objects.
    ///
    /// # Safety
    ///
    /// The caller must ensure the given object reference has +1 retain count.
    ///
    /// Additionally, there must be no [`Owned`] pointers or mutable
    /// references to the same object.
    ///
    /// And lastly, the object pointer must be valid as a reference (non-null,
    /// aligned, dereferencable, initialized and upholds aliasing rules, see
    /// the [`std::ptr`] module for more information).
    #[inline]
    pub unsafe fn new(ptr: *const T) -> Self {
        Self {
            // SAFETY: Upheld by the caller
            ptr: NonNull::new_unchecked(ptr as *mut T),
            phantom: PhantomData,
        }
    }

    /// Acquires a `*const` pointer to the object.
    #[inline]
    pub fn as_ptr(&self) -> *const T {
        self.ptr.as_ptr()
    }

    /// Retains the given object pointer.
    ///
    /// When dropped, the object will be released.
    ///
    /// # Safety
    ///
    /// The caller must ensure that there are no [`Owned`] pointers to the
    /// same object.
    ///
    /// Additionally, the object pointer must be valid as a reference
    /// (non-null, aligned, dereferencable, initialized and upholds aliasing
    /// rules, see the [`std::ptr`] module for more information).
    //
    // So this would be illegal:
    // ```rust
    // let owned: Owned<T> = ...;
    // // Lifetime information is discarded
    // let retained = Retained::retain(&*owned);
    // // Which means we can still mutate `Owned`:
    // let x: &mut T = &mut *owned;
    // // While we have an immutable reference
    // let y: &T = &*retained;
    // ```
    #[doc(alias = "objc_retain")]
    // Inlined since it's `objc_retain` that does the work.
    #[cfg_attr(debug_assertions, inline)]
    pub unsafe fn retain(ptr: *const T) -> Self {
        // SAFETY: The caller upholds that the pointer is valid
        let rtn = runtime::objc_retain(ptr as *mut Object) as *const T;
        debug_assert_eq!(rtn, ptr);
        Self {
            // SAFETY: Non-null upheld by the caller and `objc_retain` always
            // returns the same pointer.
            ptr: NonNull::new_unchecked(rtn as *mut T),
            phantom: PhantomData,
        }
    }

    /// TODO
    #[doc(alias = "objc_retainAutoreleasedReturnValue")]
    pub unsafe fn retain_autoreleased_return(_obj: *const T) -> Self {
        todo!()
    }

    /// Autoreleases the retained pointer, meaning that the object is not
    /// immediately released, but will be when the innermost / current
    /// autorelease pool is drained.
    #[doc(alias = "objc_autorelease")]
    #[must_use = "If you don't intend to use the object any more, just drop it as usual"]
    #[inline]
    pub fn autorelease<'p>(self, _pool: &'p AutoreleasePool) -> &'p T {
        let ptr = mem::ManuallyDrop::new(self).ptr;
        // SAFETY: The `ptr` is guaranteed to be valid and have at least one
        // retain count.
        // And because of the ManuallyDrop, we don't call the Drop
        // implementation, so the object won't also be released there.
        unsafe { runtime::objc_autorelease(ptr.as_ptr() as *mut Object) };
        // SAFETY: The lifetime is bounded by the type function signature
        unsafe { &*ptr.as_ptr() }
    }

    /// TODO
    #[doc(alias = "objc_autoreleaseReturnValue")]
    pub fn autorelease_return<'p>(self, _pool: &'p AutoreleasePool) -> &'p T {
        todo!()
    }

    /// TODO
    ///
    /// Equivalent to `Retained::retain(&obj).autorelease(pool)`, but slightly
    /// more efficient.
    #[doc(alias = "objc_retainAutorelease")]
    pub unsafe fn retain_and_autorelease<'p>(_obj: *const T, _pool: &'p AutoreleasePool) -> &'p T {
        todo!()
    }

    /// TODO
    ///
    /// Equivalent to `Retained::retain(&obj).autorelease_return(pool)`, but
    /// slightly more efficient.
    #[doc(alias = "objc_retainAutoreleaseReturnValue")]
    pub unsafe fn retain_and_autorelease_return<'p>(
        _obj: *const T,
        _pool: &'p AutoreleasePool,
    ) -> &'p T {
        todo!()
    }

    #[cfg(test)] // TODO
    #[doc(alias = "retainCount")]
    pub fn retain_count(&self) -> usize {
        unsafe { msg_send![self.as_ptr() as *mut Object, retainCount] }
    }
}

// TODO: Consider something like this
// #[cfg(block)]
// impl<T: Block> Retained<T> {
//     #[doc(alias = "objc_retainBlock")]
//     pub unsafe fn retain_block(block: &T) -> Self {
//         todo!()
//     }
// }

/// `#[may_dangle]` (see [this][dropck_eyepatch]) doesn't really make sense
/// here, since we actually want to disallow creating `Retained` pointers to
/// objects that have a `Drop` implementation.
///
/// [dropck_eyepatch]: https://doc.rust-lang.org/nightly/nomicon/dropck.html#an-escape-hatch
impl<T> Drop for Retained<T> {
    /// Releases the retained object
    #[doc(alias = "objc_release")]
    #[doc(alias = "release")]
    #[inline]
    fn drop(&mut self) {
        // SAFETY: The `ptr` is guaranteed to be valid and have at least one
        // retain count
        unsafe { runtime::objc_release(self.ptr.as_ptr() as *mut Object) };
    }
}

impl<T> Clone for Retained<T> {
    /// Makes a clone of the `Retained` object.
    ///
    /// This increases the object's reference count.
    #[doc(alias = "objc_retain")]
    #[doc(alias = "retain")]
    #[inline]
    fn clone(&self) -> Self {
        // SAFETY: The `ptr` is guaranteed to be valid
        unsafe { Self::retain(self.as_ptr()) }
    }
}

impl<T> Deref for Retained<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: TODO
        unsafe { self.ptr.as_ref() }
    }
}

impl<T: PartialEq> PartialEq for Retained<T> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        &**self == &**other
    }

    #[inline]
    fn ne(&self, other: &Self) -> bool {
        &**self != &**other
    }
}

// TODO: impl PartialOrd, Ord and Eq

impl<T: fmt::Display> fmt::Display for Retained<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<T: fmt::Debug> fmt::Debug for Retained<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T> fmt::Pointer for Retained<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Pointer::fmt(&self.ptr.as_ptr(), f)
    }
}

impl<T: hash::Hash> hash::Hash for Retained<T> {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        (&**self).hash(state)
    }
}

impl<T> borrow::Borrow<T> for Retained<T> {
    fn borrow(&self) -> &T {
        &**self
    }
}

impl<T> AsRef<T> for Retained<T> {
    fn as_ref(&self) -> &T {
        &**self
    }
}

// TODO: CoerceUnsized?

impl<T> Unpin for Retained<T> {}

impl<T> From<Owned<T>> for Retained<T> {
    fn from(obj: Owned<T>) -> Self {
        // SAFETY: TODO
        unsafe { Self::new(&*obj) }
    }
}

#[cfg(test)]
mod tests {
    use core::mem::size_of;

    use super::Retained;
    use crate::runtime::Object;

    pub struct TestType {
        _data: [u8; 0], // TODO: `UnsafeCell`?
    }

    #[test]
    fn test_size_of() {
        assert_eq!(size_of::<Retained<TestType>>(), size_of::<&TestType>());
        assert_eq!(
            size_of::<Option<Retained<TestType>>>(),
            size_of::<&TestType>()
        );
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    #[test]
    fn test_clone() {
        // TODO: Maybe make a way to return `Retained` directly?
        let obj: &Object = unsafe { msg_send![class!(NSObject), new] };
        let obj: Retained<Object> = unsafe { Retained::new(obj) };
        assert!(obj.retain_count() == 1);

        let cloned = obj.clone();
        assert!(cloned.retain_count() == 2);
        assert!(obj.retain_count() == 2);

        drop(obj);
        assert!(cloned.retain_count() == 1);
    }
}
