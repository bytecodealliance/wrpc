use crate::server::exports::test::records::to_test::*;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl crate::server::exports::test::records::to_test::Handler for Component {
    async fn multiple_results(&self) -> ::std::io::Result<(u8, u16)> {
        Ok((4, 5))
    }

    async fn swap_tuple(
        &self,
        a: (u8, u32),
    ) -> ::std::io::Result<(u32, u8)> {
        Ok((a.1, a.0))
    }

    async fn roundtrip_flags1(&self, a: F1) -> ::std::io::Result<F1> {
        Ok(a)
    }

    async fn roundtrip_flags2(&self, a: F2) -> ::std::io::Result<F2> {
        Ok(a)
    }

    async fn roundtrip_flags3(
        &self,
        a: Flag8,
        b: Flag16,
        c: Flag32,
    ) -> ::std::io::Result<(Flag8, Flag16, Flag32)> {
        Ok((a, b, c))
    }

    async fn roundtrip_record1(&self, a: R1) -> ::std::io::Result<R1> {
        Ok(a)
    }

    async fn tuple1(&self, a: (u8,)) -> ::std::io::Result<(u8,)> {
        Ok((a.0,))
    }
}
