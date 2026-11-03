//@ args = ['--merge-structurally-equal-types', '-dPartialEq', '--additional-derive-ignore=kind7', '--additional-derive-ignore=kind3', '--additional-derive-ignore=kind4', '--additional-derive-ignore=t-stream']

use crate::client::test::equal_types::{blag, blah};

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let kind1 = blag::Kind1::A;
    let res1 = blag::f(wrpc, (), &kind1).await?;
    let kind6 = blah::Kind6::A;
    let res2 = blah::f(wrpc, (), &kind6).await?;
    assert_eq!(res1, res2);
    let t2 = blag::T2 {
        l: blag::T3 {
            l: kind1.clone(),
            r: kind1.clone(),
        },
        r: blah::T3 {
            l: kind1.clone(),
            r: kind1.clone(),
        },
    };
    let t1 = blag::T1 {
        l: t2.clone(),
        r: t2.clone(),
    };
    let t = blag::Tree {
        l: t1.clone(),
        r: t1.clone(),
    };
    let t_stream = blag::TStream {
        tree: t.clone(),
        stream: None,
    };
    let res1 = blag::h(wrpc, (), &t_stream).await?;
    let res2 = blah::h(wrpc, (), &t_stream).await?;
    let blah::CustomResult::Ok(res2) = res2 else {
        unreachable!()
    };
    assert_eq!(res1, res2);
    Ok(())
}
