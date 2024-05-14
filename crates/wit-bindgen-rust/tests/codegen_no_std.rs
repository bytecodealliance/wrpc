//! Like `codegen_tests` in codegen.rs, but with `no_std`.
//!
//! We use `std_feature` and don't enable the "std" feature.

#![no_std]
#![allow(unused_macros)]
#![allow(dead_code, unused_variables)]

// This test expects `"std"` to be absent.
#[cfg(feature = "std")]
fn std_enabled() -> CompileError;

extern crate alloc;

// TODO: `no_std` support
//mod codegen_tests {
//    macro_rules! codegen_test {
//        ($id:ident $name:tt $test:tt) => {
//            mod $id {
//                wit_bindgen_wrpc::generate!({
//                    path: $test,
//                    std_feature,
//                    stubs
//                });
//
//                #[test]
//                fn works() {}
//            }
//
//        };
//    }
//    test_helpers::codegen_tests!();
//}

//mod strings {
//    use alloc::string::String;
//
//    wit_bindgen_wrpc::generate!({
//        inline: "
//            package my:strings;
//            world not-used-name {
//                import cat: interface {
//                    foo: func(x: string);
//                    bar: func() -> string;
//                }
//            }
//        ",
//        std_feature,
//    });
//
//    #[allow(dead_code)]
//    async fn test(wrpc: &impl wit_bindgen_wrpc::wrpc_transport::Client) -> anyhow::Result<()> {
//        // Test the argument is `&str`.
//        cat::foo(wrpc, "hello").await?;
//
//        // Test the return type is `String`.
//        let _t: String = cat::bar(wrpc).await?;
//
//        Ok(())
//    }
//}
//
///// Like `strings` but with raw_strings`.
//mod raw_strings {
//    use alloc::vec::Vec;
//
//    wit_bindgen_wrpc::generate!({
//        inline: "
//            package raw:strings;
//            world not-used-name {
//                import cat: interface {
//                    foo: func(x: string);
//                    bar: func() -> string;
//                }
//            }
//        ",
//        raw_strings,
//        std_feature,
//    });
//
//    #[allow(dead_code)]
//    async fn test(wrpc: &impl wit_bindgen_wrpc::wrpc_transport::Client) -> anyhow::Result<()> {
//        // Test the argument is `&[u8]`.
//        cat::foo(wrpc, b"hello").await?;
//
//        // Test the return type is `Vec<u8>`.
//        let _t: Vec<u8> = cat::bar(wrpc).await?;
//
//        Ok(())
//    }
//}

mod skip {
    wit_bindgen_wrpc::generate!({
        inline: "
            package foo:foo;
            world baz {
                export exports: interface {
                    foo: func();
                    bar: func();
                }
            }
        ",
        skip: ["foo"],
        std_feature,
    });

    #[derive(Clone)]
    struct Component;

    impl<Ctx: Send> exports::exports::Handler<Ctx> for Component {
        async fn bar(&self, cx: Ctx) -> anyhow::Result<()> {
            Ok(())
        }
    }

    async fn serve_exports(wrpc: &impl wrpc_transport::Client) {
        serve(wrpc, Component, async {}).await.unwrap();
    }
}
