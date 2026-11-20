use wasmtime::Engine;
use wasmtime::component::Component;
use wrpc_wasmtime::Exports;

#[test]
fn instance_names() -> anyhow::Result<()> {
    let engine = Engine::default();
    let component = Component::new(
        &engine,
        r#"(component
            (core module $m (func (export "f")))
            (core instance $i (instantiate $m))
            (func $f (canon lift (core func $i "f")))
            (instance $x (export "f" (func $f)))
            (export "foo" (instance $x))
            (export "ns:pkg/a@1.2.3+build" (instance $x))
            (export "ns:pkg/b@0.2.1" (instance $x))
            (export "ns:pkg/c@0.0.1" (instance $x))
            (export "ns:pkg/d@1.0.0-rc.1" (instance $x))
        )"#,
    )?;
    let exports = Exports::new(&component)?;
    for instance in [
        "foo",
        "ns:pkg/a@1",
        "ns:pkg/b@0.2",
        "ns:pkg/c@0.0.1",
        "ns:pkg/d@1.0.0-rc.1",
    ] {
        assert!(exports.get(instance, "f").is_some(), "`{instance}` missing");
    }
    assert!(exports.get("ns:pkg/a@1.2.3+build", "f").is_none());
    Ok(())
}
