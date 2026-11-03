package leaf

import (
	"context"
	"encoding/binary"
	"slices"
	"sync"

	wrpc "wrpc.io/go"

	"driver/leaf/exports/test/resource_with_lists/test"
)

var (
	mu     sync.Mutex
	things [][]byte
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) Thing(ctx context.Context, l []byte) (wrpc.Own[test.Thing], error) {
	mu.Lock()
	defer mu.Unlock()
	idx := uint32(len(things))
	things = append(things, append(l, " HostThing"...))
	return binary.LittleEndian.AppendUint32(nil, idx), nil
}

func (handler) Thing_Foo(ctx context.Context, self wrpc.Borrow[test.Thing]) ([]byte, error) {
	mu.Lock()
	defer mu.Unlock()
	return append(slices.Clone(things[binary.LittleEndian.Uint32(self)]), " HostThing.foo"...), nil
}

func (handler) Thing_Bar(ctx context.Context, self wrpc.Borrow[test.Thing], l []byte) error {
	mu.Lock()
	defer mu.Unlock()
	things[binary.LittleEndian.Uint32(self)] = append(l, " HostThing.bar"...)
	return nil
}

func (handler) Thing_Baz(ctx context.Context, l []byte) ([]byte, error) {
	return append(l, " HostThing.baz"...), nil
}
