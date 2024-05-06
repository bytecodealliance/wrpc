//go:generate $WIT_BINDGEN_WRPC go --world keyvalue-server --out-dir bindings/keyvalue_server --package github.com/wrpc/wrpc/tests/go/bindings/keyvalue_server ../wit
//go:generate $WIT_BINDGEN_WRPC go --world keyvalue-client --out-dir bindings/keyvalue_client --package github.com/wrpc/wrpc/tests/go/bindings/keyvalue_client ../wit

package wrpc
