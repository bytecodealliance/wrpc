//go:generate $WIT_BINDGEN_WRPC go --world resources-server --out-dir bindings/resources_server --package github.com/wrpc/wrpc/tests/go/bindings/resources_server ../wit

package integration

type ResourcesHandler struct{}
