//go:generate $WIT_BINDGEN_WRPC go --world resources-server --out-dir bindings/resources_server --package github.com/wrpc/wrpc/tests/go/bindings/resources_server ../wit

package integration

import (
	"context"
	"fmt"
	"sync"

	"github.com/google/uuid"
	wrpc "github.com/wrpc/wrpc/go"
	"github.com/wrpc/wrpc/tests/go/bindings/resources_server/exports/wrpc_test/integration/resources"
)

type Foo struct {
	cancel context.CancelFunc
	id     uuid.UUID
}

func (Foo) Bar(ctx context.Context) (string, error) {
	return "bar", nil
}

type ResourcesHandler struct {
	sync.Map
}

func (h *ResourcesHandler) Foo(ctx context.Context) (resources.HandlerFoo, context.Context, string, error) {
	id, err := uuid.NewV7()
	if err != nil {
		return nil, nil, "", fmt.Errorf("failed to generate UUIDv7: %w", err)
	}
	ctx, cancel := context.WithCancel(ctx)
	v := Foo{id: id, cancel: cancel}
	h.Store(id.String(), v)
	go func() {
		<-ctx.Done()
		h.Delete(id)
	}()
	return v, ctx, id.String(), nil
}

func (h *ResourcesHandler) FooFoo(ctx context.Context, v wrpc.Own[resources.Foo]) (string, error) {
	stored, ok := h.Load(string(v))
	if !ok {
		return "", fmt.Errorf("unknown resource ID `%s`", string(v))
	}
	foo := stored.(Foo)
	foo.cancel()
	return "foo", nil
}

func (h *ResourcesHandler) Bar(ctx context.Context, v wrpc.Borrow[resources.Foo]) (string, error) {
	stored, ok := h.Load(string(v))
	if !ok {
		return "", fmt.Errorf("unknown resource ID `%s`", string(v))
	}
	return stored.(Foo).Bar(ctx)
}

type ResourcesStrangeHandler struct{}

func (h ResourcesStrangeHandler) Bar(ctx context.Context, v wrpc.Borrow[resources.Foo]) (uint64, error) {
	return 42, nil
}
