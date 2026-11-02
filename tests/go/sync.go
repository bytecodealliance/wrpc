//go:generate $WIT_BINDGEN_WRPC go --gofmt=false --world sync-server --out-dir bindings/sync_server --package wrpc.io/tests/go/bindings/sync_server ../wit

package integration

import (
	"context"
	"fmt"
	"log/slog"
	"sync"

	"github.com/google/uuid"
	wrpc "wrpc.io/go"
	"wrpc.io/tests/go/bindings/sync_server/exports/wrpc_test/integration/resources"
	server "wrpc.io/tests/go/bindings/sync_server/exports/wrpc_test/integration/sync"
)

type SyncHandler struct{}

func (SyncHandler) Ping(ctx context.Context) (string, error) {
	return "pong", nil
}

func (SyncHandler) F(ctx context.Context, x string) (uint32, error) {
	slog.DebugContext(ctx, "handling `f`", "x", x)
	if x != "f" {
		panic(fmt.Errorf("expected: `f`\ngot:`%s`", x))
	}
	return 42, nil
}

func (SyncHandler) Foo(ctx context.Context, x string) error {
	slog.DebugContext(ctx, "handling `foo`", "x", x)
	if x != "foo" {
		return fmt.Errorf("expected: `foo`\ngot:`%s`", x)
	}
	return nil
}

func (SyncHandler) Fallible(ctx context.Context, ok bool) (*wrpc.Result[bool, string], error) {
	slog.DebugContext(ctx, "handling `fallible`", "ok", ok)
	if ok {
		return wrpc.Ok[string](true), nil
	} else {
		return wrpc.Err[bool]("test"), nil
	}
}

func (SyncHandler) Numbers(ctx context.Context) (uint8, uint16, uint32, uint64, int8, int16, int32, int64, float32, float64, error) {
	return 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, nil
}

func (SyncHandler) WithFlags(ctx context.Context, a, b, c bool) (*server.Abc, error) {
	slog.DebugContext(ctx, "handling `with-flags`", "a", a, "b", b, "c", c)
	return &server.Abc{A: a, B: b, C: c}, nil
}

func (SyncHandler) WithVariantList(ctx context.Context) ([]*server.Var, error) {
	slog.DebugContext(ctx, "handling `with-variant-list`")
	return []*server.Var{
		server.NewVarEmpty(),
		server.NewVarVar(&server.Rec{
			Nested: &server.RecNested{
				Foo: "foo",
			},
		}),
		server.NewVarEmpty(),
		server.NewVarEmpty(),
		server.NewVarEmpty(),
		server.NewVarVar(&server.Rec{
			Nested: &server.RecNested{
				Foo: "bar",
			},
		}),
	}, nil
}

func (SyncHandler) WithVariantOption(ctx context.Context, ok bool) (*server.Var, error) {
	slog.DebugContext(ctx, "handling `with-variant-option`", "ok", ok)
	if ok {
		return server.NewVarVar(&server.Rec{
			Nested: &server.RecNested{
				Foo: "bar",
			},
		}), nil
	}
	return nil, nil
}

func (SyncHandler) WithRecord(ctx context.Context) (*server.Rec, error) {
	slog.DebugContext(ctx, "handling `with-record`")
	return &server.Rec{
		Nested: &server.RecNested{
			Foo: "foo",
		},
	}, nil
}

func (SyncHandler) WithRecordList(ctx context.Context, n uint8) ([]*server.Rec, error) {
	slog.DebugContext(ctx, "handling `with-record-list`", "n", n)
	if n == 0 {
		return nil, nil
	}
	vs := make([]*server.Rec, n)
	for i := range vs {
		vs[i] = &server.Rec{
			Nested: &server.RecNested{
				Foo: fmt.Sprintf("%d", i),
			},
		}
	}
	return vs, nil
}

func (SyncHandler) WithRecordTuple(ctx context.Context) (*server.Rec, *server.Rec, error) {
	slog.DebugContext(ctx, "handling `with-record-tuple`")
	return &server.Rec{
			Nested: &server.RecNested{
				Foo: "0",
			},
		},
		&server.Rec{
			Nested: &server.RecNested{
				Foo: "1",
			},
		},
		nil
}

func (SyncHandler) WithEnum(ctx context.Context) (server.Foobar, error) {
	return server.Foobar_Bar, nil
}

func (SyncHandler) IdentityChar(ctx context.Context, c rune) (rune, error) {
	return c, nil
}

func (SyncHandler) IdentityOptionLists(ctx context.Context, a []uint8, b []string) ([]uint8, []string, error) {
	return a, b, nil
}

type Foo struct{}

func (Foo) Bar(ctx context.Context) (string, error) {
	return "bar", nil
}

type ResourcesHandler struct {
	sync.Map
	counters sync.Map
}

func (h *ResourcesHandler) Foo(ctx context.Context) (wrpc.Own[resources.Foo], error) {
	id, err := uuid.NewV7()
	if err != nil {
		return nil, fmt.Errorf("failed to generate UUIDv7: %w", err)
	}
	h.Store(id.String(), Foo{})
	return wrpc.Own[resources.Foo](id.String()), nil
}

func (h *ResourcesHandler) Foo_Foo(ctx context.Context, v wrpc.Own[resources.Foo]) (string, error) {
	if _, ok := h.LoadAndDelete(string(v)); !ok {
		return "", fmt.Errorf("unknown resource ID `%s`", string(v))
	}
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
	c, ok := h.counters.Load(v)
	if !ok {
		return nil, fmt.Errorf("unknown counter ID `%s`", v)
	}
	return c.(*counter), nil
}

func (h *ResourcesHandler) Counter(ctx context.Context, initial uint32) (wrpc.Own[resources.Counter], error) {
	id, err := uuid.NewV7()
	if err != nil {
		return nil, fmt.Errorf("failed to generate UUIDv7: %w", err)
	}
	h.counters.Store(id.String(), &counter{n: initial})
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

type StrangeHandler struct {
	Resources *ResourcesHandler
}

func (h StrangeHandler) Bar(ctx context.Context, v wrpc.Borrow[resources.Foo]) (uint64, error) {
	if _, ok := h.Resources.Load(string(v)); !ok {
		return 0, fmt.Errorf("unknown resource ID `%s`", string(v))
	}
	return 42, nil
}
