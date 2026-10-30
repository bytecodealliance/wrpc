//go:generate $WIT_BINDGEN_WRPC go --world resources-server --out-dir bindings/resources_server --package wrpc.io/tests/go/bindings/resources_server ../wit

package integration

import (
	"context"
	"fmt"
	"sync"

	"github.com/google/uuid"
	wrpc "wrpc.io/go"
	"wrpc.io/tests/go/bindings/resources_server/exports/wrpc_test/integration/resources"
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

func (h *ResourcesHandler) Foo(ctx context.Context) (wrpc.Own[resources.Foo], error) {
	id, err := uuid.NewV7()
	if err != nil {
		return nil, fmt.Errorf("failed to generate UUIDv7: %w", err)
	}
	ctx, cancel := context.WithCancel(ctx)
	v := Foo{id: id, cancel: cancel}
	h.Store(id.String(), v)
	go func() {
		<-ctx.Done()
		h.Delete(id)
	}()
	return wrpc.Own[resources.Foo](id.String()), nil
}

func (h *ResourcesHandler) Foo_Foo(ctx context.Context, v wrpc.Own[resources.Foo]) (string, error) {
	stored, ok := h.Load(string(v))
	if !ok {
		return "", fmt.Errorf("unknown resource ID `%s`", string(v))
	}
	foo := stored.(Foo)
	foo.cancel()
	return "foo", nil
}

func (h *ResourcesHandler) Foo_Bar(ctx context.Context, v wrpc.Borrow[resources.Foo]) (string, error) {
	stored, ok := h.Load(string(v))
	if !ok {
		return "", fmt.Errorf("unknown resource ID `%s`", string(v))
	}
	return stored.(Foo).Bar(ctx)
}

func (h *ResourcesHandler) Bar(ctx context.Context, v wrpc.Borrow[resources.Foo]) (string, error) {
	stored, ok := h.Load(string(v))
	if !ok {
		return "", fmt.Errorf("unknown resource ID `%s`", string(v))
	}
	return stored.(Foo).Bar(ctx)
}

type counter struct {
	sync.Mutex
	n uint32
}

func (h *ResourcesHandler) counter(v string) (*counter, error) {
	stored, ok := h.Load(v)
	if !ok {
		return nil, fmt.Errorf("unknown resource ID `%s`", v)
	}
	c, ok := stored.(*counter)
	if !ok {
		return nil, fmt.Errorf("resource `%s` is not a counter", v)
	}
	return c, nil
}

func (h *ResourcesHandler) Counter(ctx context.Context, initial uint32) (wrpc.Own[resources.Counter], error) {
	id, err := uuid.NewV7()
	if err != nil {
		return nil, fmt.Errorf("failed to generate UUIDv7: %w", err)
	}
	h.Store(id.String(), &counter{n: initial})
	return wrpc.Own[resources.Counter](id.String()), nil
}

func (h *ResourcesHandler) Counter_CloneCounter(ctx context.Context, self wrpc.Borrow[resources.Counter]) (wrpc.Own[resources.Counter], error) {
	n, err := h.Counter_GetCount(ctx, self)
	if err != nil {
		return nil, err
	}
	return h.Counter(ctx, n)
}

func (h *ResourcesHandler) Counter_GetCount(ctx context.Context, self wrpc.Borrow[resources.Counter]) (uint32, error) {
	c, err := h.counter(string(self))
	if err != nil {
		return 0, err
	}
	c.Lock()
	defer c.Unlock()
	return c.n, nil
}

func (h *ResourcesHandler) Counter_IncrementBy(ctx context.Context, self wrpc.Borrow[resources.Counter], num uint32) error {
	c, err := h.counter(string(self))
	if err != nil {
		return err
	}
	c.Lock()
	defer c.Unlock()
	c.n += num
	return nil
}

func (h *ResourcesHandler) Counter_Sum(ctx context.Context, a wrpc.Borrow[resources.Counter], b wrpc.Borrow[resources.Counter]) (uint32, error) {
	x, err := h.Counter_GetCount(ctx, a)
	if err != nil {
		return 0, err
	}
	y, err := h.Counter_GetCount(ctx, b)
	if err != nil {
		return 0, err
	}
	return x + y, nil
}

type ResourcesStrangeHandler struct{}

func (h ResourcesStrangeHandler) Bar(ctx context.Context, v wrpc.Borrow[resources.Foo]) (uint64, error) {
	return 42, nil
}
