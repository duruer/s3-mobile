//! Typed views of guest memory.
//!
//! - [`GuestPtr<T>`]: a guest address of a plain value (`u8/u16/u32/i8/i16/i32/f32`).
//! - [`guest_struct!`](crate::guest_struct): declares a host-side view type for a guest struct with field
//!   offsets, so mod code reads `player.wood(mem)` instead of `mem.read_u16(base + 0x24)`.
//!
//! ```
//! s3_mods::guest_struct! {
//!     /// Example (offsets are made up).
//!     pub struct Example {
//!         0x00 => id / set_id: u32,
//!         0x04 => hp / set_hp: i16,
//!     }
//! }
//! let mut mem = s3_mem::GuestMemory::new().unwrap();
//! let e = Example::at(0x0060_0000);
//! e.set_hp(&mut mem, -3);
//! assert_eq!(e.hp(&mem), -3);
//! ```

use std::marker::PhantomData;

use s3_mem::GuestMemory;

/// A value type that can live in guest memory (little endian).
pub trait GuestValue: Copy {
    const SIZE: u32;
    fn read(mem: &GuestMemory, addr: u32) -> Self;
    fn write(self, mem: &mut GuestMemory, addr: u32);
}

macro_rules! gv {
    ($t:ty, $n:expr, $r:ident, $w:ident, $cast_r:ty) => {
        impl GuestValue for $t {
            const SIZE: u32 = $n;
            #[inline]
            fn read(mem: &GuestMemory, addr: u32) -> Self {
                mem.$r(addr) as $cast_r as $t
            }
            #[inline]
            fn write(self, mem: &mut GuestMemory, addr: u32) {
                mem.$w(addr, self as $cast_r);
            }
        }
    };
}
gv!(u8, 1, read_u8, write_u8, u8);
gv!(i8, 1, read_u8, write_u8, u8);
gv!(u16, 2, read_u16, write_u16, u16);
gv!(i16, 2, read_u16, write_u16, u16);
gv!(u32, 4, read_u32, write_u32, u32);
gv!(i32, 4, read_u32, write_u32, u32);

impl GuestValue for f32 {
    const SIZE: u32 = 4;
    fn read(mem: &GuestMemory, addr: u32) -> Self {
        f32::from_bits(mem.read_u32(addr))
    }
    fn write(self, mem: &mut GuestMemory, addr: u32) {
        mem.write_u32(addr, self.to_bits());
    }
}

/// Guest address of a `T`.
#[derive(Debug, PartialEq, Eq)]
pub struct GuestPtr<T> {
    pub addr: u32,
    _t: PhantomData<T>,
}

impl<T> Clone for GuestPtr<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for GuestPtr<T> {}

impl<T: GuestValue> GuestPtr<T> {
    pub const fn new(addr: u32) -> Self {
        GuestPtr { addr, _t: PhantomData }
    }
    pub fn is_null(&self) -> bool {
        self.addr == 0
    }
    pub fn get(&self, mem: &GuestMemory) -> T {
        T::read(mem, self.addr)
    }
    pub fn set(&self, mem: &mut GuestMemory, v: T) {
        v.write(mem, self.addr)
    }
    /// Element `i` of an array starting here.
    pub fn index(&self, i: u32) -> GuestPtr<T> {
        GuestPtr::new(self.addr.wrapping_add(i.wrapping_mul(T::SIZE)))
    }
}

/// Declare a view of a guest struct. Each field: `offset => getter / setter: type` (type implements
/// [`GuestValue`]). Generates `Name::at(addr)`, `Name::addr()`, the getters/setters and `Name::ptr_<getter>`
/// is not generated (use `GuestPtr::new(v.addr() + off)` for raw pointers).
#[macro_export]
macro_rules! guest_struct {
    ($(#[$meta:meta])* $vis:vis struct $name:ident { $($off:literal => $get:ident / $set:ident : $t:ty),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        $vis struct $name { base: u32 }
        #[allow(dead_code)]
        impl $name {
            pub const fn at(base: u32) -> Self { $name { base } }
            pub const fn addr(&self) -> u32 { self.base }
            $(
                pub fn $get(&self, mem: &$crate::s3_mem::GuestMemory) -> $t {
                    <$t as $crate::view::GuestValue>::read(mem, self.base.wrapping_add($off))
                }
                pub fn $set(&self, mem: &mut $crate::s3_mem::GuestMemory, v: $t) {
                    <$t as $crate::view::GuestValue>::write(v, mem, self.base.wrapping_add($off))
                }
            )*
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    crate::guest_struct! {
        struct Demo {
            0x0 => a / set_a: u32,
            0x4 => b / set_b: i8,
            0x6 => c / set_c: u16,
            0x8 => f / set_f: f32,
        }
    }

    #[test]
    fn struct_and_ptr() {
        let mut mem = GuestMemory::new().unwrap();
        let d = Demo::at(0x0060_0000);
        d.set_a(&mut mem, 0xDEAD_BEEF);
        d.set_b(&mut mem, -2);
        d.set_c(&mut mem, 0x1234);
        d.set_f(&mut mem, 1.5);
        assert_eq!(d.a(&mem), 0xDEAD_BEEF);
        assert_eq!(d.b(&mem), -2);
        assert_eq!(mem.read_u8(0x0060_0004), 0xFE);
        assert_eq!(d.c(&mem), 0x1234);
        assert_eq!(d.f(&mem), 1.5);
        let p: GuestPtr<u16> = GuestPtr::new(0x0060_0000);
        assert_eq!(p.index(3).get(&mem), 0x1234);
        p.index(3).set(&mut mem, 7);
        assert_eq!(d.c(&mem), 7);
    }
}
