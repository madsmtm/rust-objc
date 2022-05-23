#[macro_use]
extern crate objc; // objc v0.2.7

use objc::runtime::{Object, Sel};

extern "C" fn my_selector(obj: *mut Object, _sel: Sel) {
    let obj = unsafe { &mut *obj };
    let a = unsafe { obj.get_mut_ivar::<i32>("a") };
    *a += 1;
}

fn main() {
    let ptr: *mut Object = new_object();
    let obj: &mut Object = unsafe { &mut *ptr };

    // Get an immutable reference to an instance variable
    let a = unsafe { obj.get_ivar::<i32>("a") };

    unsafe {
        // Uses `obj` mutably, but the signature says it's used immutably
        let _: () = msg_send![obj, my_selector];
    }
    // So the compiler can't catch that we're not allowed to access `a` here!
    assert_eq!(*a, 43);

    free_object(ptr);
}

// ------------------------------------
//
// HACKY STUBS BELOW TO MAKE MIRI WORK!
//
// ------------------------------------

use std::ffi::CStr;
use std::os::raw::c_char;
use std::ptr;

use objc::runtime::{Class, Ivar};

#[repr(C)]
struct MyObject {
    isa: *const Class,
    a: i32,
}

fn new_object() -> *mut Object {
    let obj = Box::new(MyObject {
        isa: ptr::null(),
        a: 42,
    });
    Box::into_raw(obj) as *mut Object
}

fn free_object(obj: *mut Object) {
    unsafe { Box::from_raw(obj as *mut MyObject) };
}

#[no_mangle]
extern "C" fn sel_registerName(name: *const c_char) -> Sel {
    unsafe { Sel::from_ptr(name.cast()) }
}

#[no_mangle]
extern "C" fn objc_msgSend(obj: *mut Object, sel: Sel) {
    my_selector(obj, sel)
}

#[no_mangle]
extern "C" fn object_getClass(obj: *const Object) -> *const Class {
    // Must be a valid pointer, so don't return isa
    obj.cast()
}

#[no_mangle]
extern "C" fn class_getInstanceVariable(cls: *const Class, _name: *const c_char) -> *const Ivar {
    cls.cast()
}

#[no_mangle]
extern "C" fn ivar_getTypeEncoding(_ivar: *const Ivar) -> *const c_char {
    CStr::from_bytes_with_nul(b"i\0").unwrap().as_ptr()
}

#[no_mangle]
extern "C" fn ivar_getOffset(_ivar: *const Ivar) -> isize {
    8
}
