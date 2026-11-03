package leaf

import (
	"context"
	"encoding/binary"
	"sync"

	wrpc "wrpc.io/go"

	"driver/leaf/exports/imports"
)

var (
	mu sync.Mutex
	ys []int32
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) Y(ctx context.Context, a int32) (wrpc.Own[imports.Y], error) {
	mu.Lock()
	defer mu.Unlock()
	idx := uint32(len(ys))
	ys = append(ys, a)
	return binary.LittleEndian.AppendUint32(nil, idx), nil
}

func (handler) Y_GetA(ctx context.Context, self wrpc.Borrow[imports.Y]) (int32, error) {
	mu.Lock()
	defer mu.Unlock()
	return ys[binary.LittleEndian.Uint32(self)], nil
}

func (handler) Y_SetA(ctx context.Context, self wrpc.Borrow[imports.Y], a int32) error {
	mu.Lock()
	defer mu.Unlock()
	ys[binary.LittleEndian.Uint32(self)] = a
	return nil
}

func (h handler) Y_Add(ctx context.Context, y wrpc.Own[imports.Y], a int32) (wrpc.Own[imports.Y], error) {
	mu.Lock()
	a += ys[binary.LittleEndian.Uint32(y)]
	mu.Unlock()
	return h.Y(ctx, a)
}
