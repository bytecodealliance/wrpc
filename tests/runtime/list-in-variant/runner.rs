use crate::client::test::list_in_variant::to_test::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let hw: Vec<&str> = vec!["hello", "world"];
    let (mut tx, mut rx) = wrpc.connect().await?;
    assert_eq!(
        list_in_option(&mut tx, &mut rx, Some(hw.as_slice())).await?,
        "hello,world"
    );
    assert_eq!(list_in_option(&mut tx, &mut rx, None).await?, "none");

    let fbb = PayloadOrEmpty::WithData(vec!["foo".into(), "bar".into(), "baz".into()]);
    assert_eq!(list_in_variant(&mut tx, &mut rx, &fbb).await?, "foo,bar,baz");
    assert_eq!(
        list_in_variant(&mut tx, &mut rx, &PayloadOrEmpty::Empty).await?,
        "empty"
    );

    let abc: Result<Vec<String>, String> = Ok(vec!["a".into(), "b".into(), "c".into()]);
    assert_eq!(list_in_result(&mut tx, &mut rx, &abc).await?, "a,b,c");
    let oops: Result<Vec<String>, String> = Err("oops".to_string());
    assert_eq!(list_in_result(&mut tx, &mut rx, &oops).await?, "err:oops");

    let hw2: Vec<&str> = vec!["hello", "world"];
    let s = list_in_option_with_return(&mut tx, &mut rx, Some(hw2.as_slice())).await?;
    assert_eq!(s.count, 2);
    assert_eq!(s.label, "hello,world");
    let s = list_in_option_with_return(&mut tx, &mut rx, None).await?;
    assert_eq!(s.count, 0);
    assert_eq!(s.label, "none");

    let xyz: Vec<&str> = vec!["x", "y", "z"];
    assert_eq!(top_level_list(&mut tx, &mut rx, &xyz).await?, "x,y,z");
    Ok(())
}
