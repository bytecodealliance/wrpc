use core::any;
use core::fmt::{self, Debug};
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;
use core::task::{Context, Poll};

use bytes::{Bytes, BytesMut};
use tracing::instrument;
use wasm_tokio::{CoreVecDecoder, CoreVecDecoderBytes, CoreVecEncoderBytes};

use crate::value::sync_receive;
use crate::{Decode, Encode, Receive, ReceiveError, error_unknown_path};

macro_rules! impl_resource {
    ($(#[$attr:meta])* $name:ident, $kind:literal) => {
        $(#[$attr])*
        #[repr(transparent)]
        pub struct $name<T: ?Sized> {
            repr: Bytes,
            _ty: PhantomData<T>,
        }

        impl<T: ?Sized> From<Bytes> for $name<T> {
            fn from(repr: Bytes) -> Self {
                Self {
                    repr,
                    _ty: PhantomData,
                }
            }
        }

        impl<T: ?Sized> From<Vec<u8>> for $name<T> {
            fn from(repr: Vec<u8>) -> Self {
                Self::from(Bytes::from(repr))
            }
        }

        impl<T: ?Sized> From<$name<T>> for Bytes {
            fn from($name { repr, .. }: $name<T>) -> Self {
                repr
            }
        }

        impl<T: ?Sized> PartialEq for $name<T> {
            fn eq(&self, other: &Self) -> bool {
                self.repr == other.repr
            }
        }

        impl<T: ?Sized> Eq for $name<T> {}

        impl<T: ?Sized> Hash for $name<T> {
            fn hash<H: Hasher>(&self, state: &mut H) {
                self.repr.hash(state);
            }
        }

        impl<T: ?Sized> AsRef<[u8]> for $name<T> {
            fn as_ref(&self) -> &[u8] {
                &self.repr
            }
        }

        impl<T: ?Sized> AsRef<Bytes> for $name<T> {
            fn as_ref(&self) -> &Bytes {
                &self.repr
            }
        }

        impl<T: ?Sized> Debug for $name<T> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}<{}>", $kind, any::type_name::<T>())
            }
        }

        impl<T: ?Sized> Clone for $name<T> {
            fn clone(&self) -> Self {
                Self::from(self.repr.clone())
            }
        }

        impl<T: ?Sized> $name<T> {
            /// Constructs a new resource handle
            pub fn new(repr: impl Into<Bytes>) -> Self {
                Self::from(repr.into())
            }
        }
    };
}

impl_resource!(
    /// Borrowed resource handle, represented as an opaque byte blob
    ResourceBorrow,
    "borrow"
);

impl_resource!(
    /// Owned resource handle, represented as an opaque byte blob
    ResourceOwn,
    "own"
);

impl<T: ?Sized> From<ResourceOwn<T>> for ResourceBorrow<T> {
    fn from(ResourceOwn { repr, .. }: ResourceOwn<T>) -> Self {
        Self::from(repr)
    }
}

impl<T: ?Sized> ResourceOwn<T> {
    /// Returns the owned handle as [`ResourceBorrow`]
    pub fn as_borrow(&self) -> ResourceBorrow<T> {
        ResourceBorrow::from(self.repr.clone())
    }
}

impl<T: ?Sized> Encode for ResourceOwn<T> {
    type Encoder = CoreVecEncoderBytes;
}

impl<T: ?Sized> Encode for &ResourceOwn<T> {
    type Encoder = CoreVecEncoderBytes;
}

impl<T: ?Sized> Encode for ResourceBorrow<T> {
    type Encoder = CoreVecEncoderBytes;
}

impl<T: ?Sized> Encode for &ResourceBorrow<T> {
    type Encoder = CoreVecEncoderBytes;
}

/// Decoder for resource handles of type `T`
#[derive(Debug)]
pub struct ResourceDecoder<T> {
    dec: CoreVecDecoderBytes,
    _ty: PhantomData<fn() -> T>,
}

impl<T> Default for ResourceDecoder<T> {
    fn default() -> Self {
        Self {
            dec: CoreVecDecoderBytes::default(),
            _ty: PhantomData,
        }
    }
}

impl<T> Receive for ResourceDecoder<T> {
    sync_receive!();
}

impl<T: From<Bytes>> Receive for CoreVecDecoder<ResourceDecoder<T>> {
    sync_receive!();
}

impl<T: From<Bytes>> tokio_util::codec::Decoder for ResourceDecoder<T> {
    type Item = T;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self), fields(ty = "resource"))]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let repr = self.dec.decode(src)?;
        Ok(repr.map(T::from))
    }
}

impl<T: ?Sized + 'static> Decode for ResourceOwn<T> {
    type Decoder = ResourceDecoder<Self>;
    type ListDecoder = CoreVecDecoder<Self::Decoder>;
}

impl<T: ?Sized + 'static> Decode for ResourceBorrow<T> {
    type Decoder = ResourceDecoder<Self>;
    type ListDecoder = CoreVecDecoder<Self::Decoder>;
}
