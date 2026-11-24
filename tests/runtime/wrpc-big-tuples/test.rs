use core::future::Future;
use core::pin::Pin;

use crate::server::exports::wrpc_test::big_tuples::to_test::{Handler, T17};

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn roundtrip(&self, a: Vec<T17>) -> ::std::io::Result<Vec<T17>> {
        Ok(a)
    }

    async fn nested_future(
        &self,
        a: Tuple17<
            u8,
            u8,
            u8,
            u8,
            u8,
            u8,
            u8,
            u8,
            u8,
            u8,
            u8,
            u8,
            u8,
            u8,
            u8,
            u8,
            Pin<Box<dyn Future<Output = u8> + Send>>,
        >,
    ) -> ::std::io::Result<Pin<Box<dyn Future<Output = u8> + Send>>> {
        Ok(a.16)
    }

    async fn many_params(
        &self,
        a0: u8,
        a1: u8,
        a2: u8,
        a3: u8,
        a4: u8,
        a5: u8,
        a6: u8,
        a7: u8,
        a8: u8,
        a9: u8,
        a10: u8,
        a11: u8,
        a12: u8,
        a13: u8,
        a14: u8,
        a15: u8,
        a16: String,
    ) -> ::std::io::Result<String> {
        assert_eq!(
            [
                a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13, a14, a15
            ],
            [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
        );
        Ok(a16)
    }
}
