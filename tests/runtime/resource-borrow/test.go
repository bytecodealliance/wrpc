package test

import (
	"context"
	"encoding/binary"

	wrpc "wrpc.io/go"

	to_test "driver/test/exports/test/resource_borrow/to_test"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) Thing(ctx context.Context, v uint32) (wrpc.Own[to_test.Thing], error) {
	buf := make([]byte, 4)
	binary.LittleEndian.PutUint32(buf, v+1)
	return wrpc.Own[to_test.Thing](buf), nil
}

func (handler) Foo(ctx context.Context, v wrpc.Borrow[to_test.Thing]) (uint32, error) {
	val := binary.LittleEndian.Uint32([]byte(v))
	return val + 2, nil
}
