use super::Rng;

macro_rules! unsigned_gen {
    ($name:ident, $u:ty, $wide:ty) => {
        pub fn $name<R: Rng + ?Sized>(rng: &mut R, s: $u) -> $u {
            let mut x = rng.next_u64() as $u;
            if s == 0 {
                return x;
            }
            let mut m = <$wide>::from(x) * <$wide>::from(s);
            let mut l = m as $u;
            if l < s {
                let t =
                    ((<$wide>::from(<$u>::MAX) - <$wide>::from(s - 1)) % <$wide>::from(s)) as $u;
                while l < t {
                    x = rng.next_u64() as $u;
                    m = <$wide>::from(x) * <$wide>::from(s);
                    l = m as $u;
                }
            }
            (m >> <$u>::BITS) as $u
        }
    };
}

unsigned_gen!(uniform_u8, u8, u16);
unsigned_gen!(uniform_u16, u16, u32);
unsigned_gen!(uniform_u32, u32, u64);
unsigned_gen!(uniform_u64, u64, u128);

macro_rules! signed_gen {
    ($name:ident, $uniform_u:ident, $u:ty, $i:ty) => {
        pub fn $name<R: Rng + ?Sized>(rng: &mut R, s: $u) -> $i {
            if s == 0 {
                return $uniform_u(rng, 0) as $i;
            }
            $uniform_u(rng, s.wrapping_mul(2).wrapping_sub(1)).wrapping_sub(s - 1) as $i
        }
    };
}

signed_gen!(uniform_i8, uniform_u8, u8, i8);
signed_gen!(uniform_i16, uniform_u16, u16, i16);
signed_gen!(uniform_i32, uniform_u32, u32, i32);
signed_gen!(uniform_i64, uniform_u64, u64, i64);

macro_rules! range_gen {
    ($range_u:ident, $range_i:ident, $uniform_u:ident, $u:ty, $i:ty) => {
        pub fn $range_u<R: Rng + ?Sized>(rng: &mut R, min: $u, max: $u) -> $u {
            if min == max {
                return min;
            }
            let width = max.wrapping_sub(min).wrapping_add(1);
            $uniform_u(rng, width).wrapping_add(min)
        }

        pub fn $range_i<R: Rng + ?Sized>(rng: &mut R, min: $i, max: $i) -> $i {
            if min == max {
                return min;
            }
            if min == <$i>::MIN && max == <$i>::MAX {
                return $uniform_u(rng, 0) as $i;
            }
            let width = (max.wrapping_sub(min) as $u).wrapping_add(1);
            $uniform_u(rng, width).wrapping_add(min as $u) as $i
        }
    };
}

range_gen!(range_u8, range_i8, uniform_u8, u8, i8);
range_gen!(range_u16, range_i16, uniform_u16, u16, i16);
range_gen!(range_u32, range_i32, uniform_u32, u32, i32);
range_gen!(range_u64, range_i64, uniform_u64, u64, i64);
