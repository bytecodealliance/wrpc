package leaf_thing

import (
	"context"
	"encoding/binary"
	"sync"

	wrpc "wrpc.io/go"

	"driver/leaf_thing/exports/test/resource_import_and_export/test"
)

var (
	mu     sync.Mutex
	things []uint32
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) Thing(ctx context.Context, v uint32) (wrpc.Own[test.Thing], error) {
	mu.Lock()
	defer mu.Unlock()
	idx := uint32(len(things))
	things = append(things, v+1)
	return binary.LittleEndian.AppendUint32(nil, idx), nil
}

func (handler) Thing_Foo(ctx context.Context, self wrpc.Borrow[test.Thing]) (uint32, error) {
	mu.Lock()
	defer mu.Unlock()
	return things[binary.LittleEndian.Uint32(self)] + 2, nil
}

func (handler) Thing_Bar(ctx context.Context, self wrpc.Borrow[test.Thing], v uint32) error {
	mu.Lock()
	defer mu.Unlock()
	things[binary.LittleEndian.Uint32(self)] = v + 3
	return nil
}

func (h handler) Thing_Baz(ctx context.Context, a wrpc.Own[test.Thing], b wrpc.Own[test.Thing]) (wrpc.Own[test.Thing], error) {
	mu.Lock()
	v := things[binary.LittleEndian.Uint32(a)] + 2 + things[binary.LittleEndian.Uint32(b)] + 2 + 4
	mu.Unlock()
	return h.Thing(ctx, v)
}
