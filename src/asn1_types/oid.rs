use crate::{Any, CheckDerConstraints, Error, FromBer, FromDer, ParseResult, Result, Tag, Tagged};
use std::{borrow::Cow, convert::TryFrom, fmt, iter::FusedIterator, ops::Shl, str::FromStr};

#[cfg(feature = "bigint")]
use num_bigint::BigUint;
use num_traits::Num;

#[derive(Debug)]
pub enum ParseError {
    TooShort,
    /// Signalizes that the first or second component is too large.
    /// The first must be within the range 0 to 6 (inclusive).
    /// The second component must be less than 40.
    FirstComponentsTooLarge,
    ParseIntError,
}

/// Object ID (OID) representation which can be relative or non-relative.
/// An example for an oid in string representation is "1.2.840.113549.1.1.5".
///
/// For non-relative oids restrictions apply to the first two components.
///
/// This library contains a procedural macro `oid` which can be used to
/// create oids. For example `oid!(1.2.44.233)` or `oid!(rel 44.233)`
/// for relative oids. See the [module documentation](index.html) for more information.
#[derive(Hash, PartialEq, Eq, Clone)]

pub struct Oid<'a> {
    asn1: Cow<'a, [u8]>,
    relative: bool,
}

impl<'a> TryFrom<Any<'a>> for Oid<'a> {
    type Error = Error;

    fn try_from(any: Any<'a>) -> Result<Self> {
        // check that any.data.last().unwrap() >> 7 == 0u8
        let asn1 = any.into_cow();
        Ok(Oid::new(asn1))
    }
}

impl<'a> CheckDerConstraints for Oid<'a> {
    fn check_constraints(any: &Any) -> Result<()> {
        any.header.assert_primitive()?;
        any.header.length.assert_definite()?;
        Ok(())
    }
}

impl<'a> Tagged for Oid<'a> {
    const TAG: Tag = Tag::Oid;
}

fn encode_relative(ids: &'_ [u64]) -> impl Iterator<Item = u8> + '_ {
    ids.iter()
        .map(|id| {
            let bit_count = 64 - id.leading_zeros();
            let octets_needed = ((bit_count + 6) / 7).max(1);
            (0..octets_needed).map(move |i| {
                let flag = if i == octets_needed - 1 { 0 } else { 1 << 7 };
                ((id >> (7 * (octets_needed - 1 - i))) & 0b111_1111) as u8 | flag
            })
        })
        .flatten()
}

impl<'a> Oid<'a> {
    /// Create an OID from the ASN.1 DER encoded form. See the [module documentation](index.html)
    /// for other ways to create oids.
    pub const fn new(asn1: Cow<'a, [u8]>) -> Oid {
        Oid {
            asn1,
            relative: false,
        }
    }

    /// Create a relative OID from the ASN.1 DER encoded form. See the [module documentation](index.html)
    /// for other ways to create relative oids.
    pub const fn new_relative(asn1: Cow<'a, [u8]>) -> Oid {
        Oid {
            asn1,
            relative: true,
        }
    }

    /// Build an OID from an array of object identifier components.
    /// This method allocates memory on the heap.
    pub fn from<'b>(s: &'b [u64]) -> std::result::Result<Oid<'static>, ParseError> {
        if s.len() < 2 {
            if s.len() == 1 && s[0] == 0 {
                return Ok(Oid {
                    asn1: Cow::Borrowed(&[0]),
                    relative: false,
                });
            }
            return Err(ParseError::TooShort);
        }
        if s[0] >= 7 || s[1] >= 40 {
            return Err(ParseError::FirstComponentsTooLarge);
        }
        let asn1_encoded: Vec<u8> = [(s[0] * 40 + s[1]) as u8]
            .iter()
            .copied()
            .chain(encode_relative(&s[2..]))
            .collect();
        Ok(Oid {
            asn1: Cow::from(asn1_encoded),
            relative: false,
        })
    }

    /// Build a relative OID from an array of object identifier components.
    pub fn from_relative<'b>(s: &'b [u64]) -> std::result::Result<Oid<'static>, ParseError> {
        if s.is_empty() {
            return Err(ParseError::TooShort);
        }
        let asn1_encoded: Vec<u8> = encode_relative(s).collect();
        Ok(Oid {
            asn1: Cow::from(asn1_encoded),
            relative: true,
        })
    }

    /// Create a deep copy of the oid.
    ///
    /// This method allocates data on the heap. The returned oid
    /// can be used without keeping the ASN.1 representation around.
    ///
    /// Cloning the returned oid does again allocate data.
    pub fn to_owned(&self) -> Oid<'static> {
        Oid {
            asn1: Cow::from(self.asn1.to_vec()),
            relative: self.relative,
        }
    }

    /// Get the encoded oid without the header.
    pub fn as_bytes(&self) -> &[u8] {
        self.asn1.as_ref()
    }

    /// Convert the OID to a string representation.
    /// The string contains the IDs separated by dots, for ex: "1.2.840.113549.1.1.5"
    #[cfg(feature = "bigint")]
    pub fn to_id_string(&self) -> String {
        let ints: Vec<String> = self.iter_bigint().map(|i| i.to_string()).collect();
        ints.join(".")
    }

    #[cfg(not(feature = "bigint"))]
    /// Convert the OID to a string representation.
    ///
    /// If every arc fits into a u64 a string like "1.2.840.113549.1.1.5"
    /// is returned, otherwise a hex representation.
    ///
    /// See also the "bigint" feature of this crate.
    pub fn to_id_string(&self) -> String {
        if let Some(arcs) = self.iter() {
            let ints: Vec<String> = arcs.map(|i| i.to_string()).collect();
            ints.join(".")
        } else {
            let mut ret = String::with_capacity(self.asn1.len() * 3);
            for (i, o) in self.asn1.iter().enumerate() {
                ret.push_str(&format!("{:02x}", o));
                if i + 1 != self.asn1.len() {
                    ret.push(' ');
                }
            }
            ret
        }
    }

    /// Return an iterator over the sub-identifiers (arcs).
    #[cfg(feature = "bigint")]
    pub fn iter_bigint(
        &'_ self,
    ) -> impl Iterator<Item = BigUint> + FusedIterator + ExactSizeIterator + '_ {
        SubIdentifierIterator {
            oid: &self,
            pos: 0,
            first: false,
            n: std::marker::PhantomData,
        }
    }

    /// Return an iterator over the sub-identifiers (arcs).
    /// Returns `None` if at least one arc does not fit into `u64`.
    pub fn iter(
        &'_ self,
    ) -> Option<impl Iterator<Item = u64> + FusedIterator + ExactSizeIterator + '_> {
        // Check that every arc fits into u64
        let bytes = if self.relative {
            &self.asn1
        } else if self.asn1.is_empty() {
            &[]
        } else {
            &self.asn1[1..]
        };
        let max_bits = bytes
            .iter()
            .fold((0usize, 0usize), |(max, cur), c| {
                let is_end = (c >> 7) == 0u8;
                if is_end {
                    (max.max(cur + 7), 0)
                } else {
                    (max, cur + 7)
                }
            })
            .0;
        if max_bits > 64 {
            return None;
        }

        Some(SubIdentifierIterator {
            oid: &self,
            pos: 0,
            first: false,
            n: std::marker::PhantomData,
        })
    }

    pub fn from_ber_relative(bytes: &'a [u8]) -> ParseResult<'a, Self> {
        let (rem, any) = Any::from_ber(bytes)?;
        any.header.assert_primitive()?;
        any.header.assert_tag(Tag::RelativeOid)?;
        let asn1 = any.into_cow();
        Ok((rem, Oid::new_relative(asn1)))
    }

    pub fn from_der_relative(bytes: &'a [u8]) -> ParseResult<'a, Self> {
        let (rem, any) = Any::from_der(bytes)?;
        any.header.assert_tag(Tag::RelativeOid)?;
        Self::check_constraints(&any)?;
        let asn1 = any.into_cow();
        Ok((rem, Oid::new_relative(asn1)))
    }
}

trait Repr: Num + Shl<usize, Output = Self> + From<u8> {}
impl<N> Repr for N where N: Num + Shl<usize, Output = N> + From<u8> {}

struct SubIdentifierIterator<'a, N: Repr> {
    oid: &'a Oid<'a>,
    pos: usize,
    first: bool,
    n: std::marker::PhantomData<&'a N>,
}

impl<'a, N: Repr> Iterator for SubIdentifierIterator<'a, N> {
    type Item = N;

    fn next(&mut self) -> Option<Self::Item> {
        use num_traits::identities::Zero;

        if self.pos == self.oid.asn1.len() {
            return None;
        }
        if !self.oid.relative {
            if !self.first {
                debug_assert!(self.pos == 0);
                self.first = true;
                return Some((self.oid.asn1[0] / 40).into());
            } else if self.pos == 0 {
                self.pos += 1;
                if self.oid.asn1[0] == 0 && self.oid.asn1.len() == 1 {
                    return None;
                }
                return Some((self.oid.asn1[0] % 40).into());
            }
        }
        // decode objet sub-identifier according to the asn.1 standard
        let mut res = <N as Zero>::zero();
        for o in self.oid.asn1[self.pos..].iter() {
            self.pos += 1;
            res = (res << 7) + (o & 0b111_1111).into();
            let flag = o >> 7;
            if flag == 0u8 {
                break;
            }
        }
        Some(res)
    }
}

impl<'a, N: Repr> FusedIterator for SubIdentifierIterator<'a, N> {}

impl<'a, N: Repr> ExactSizeIterator for SubIdentifierIterator<'a, N> {
    fn len(&self) -> usize {
        if self.oid.relative {
            self.oid.asn1.iter().filter(|o| (*o >> 7) == 0u8).count()
        } else if self.oid.asn1.len() == 0 {
            0
        } else if self.oid.asn1.len() == 1 {
            if self.oid.asn1[0] == 0 {
                1
            } else {
                2
            }
        } else {
            2 + self.oid.asn1[2..]
                .iter()
                .filter(|o| (*o >> 7) == 0u8)
                .count()
        }
    }

    #[cfg(feature = "exact_size_is_empty")]
    fn is_empty(&self) -> bool {
        self.oid.asn1.is_empty()
    }
}

impl<'a> fmt::Display for Oid<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.relative {
            f.write_str("rel. ")?;
        }
        f.write_str(&self.to_id_string())
    }
}

impl<'a> fmt::Debug for Oid<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("OID(")?;
        <Oid as fmt::Display>::fmt(self, f)?;
        f.write_str(")")
    }
}

impl<'a> FromStr for Oid<'a> {
    type Err = ParseError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let v: std::result::Result<Vec<_>, _> = s.split('.').map(|c| c.parse::<u64>()).collect();
        v.map_err(|_| ParseError::ParseIntError)
            .and_then(|v| Oid::from(&v))
    }
}