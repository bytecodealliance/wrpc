package test

import (
	"context"
	"fmt"

	"driver/test/exports/wrpc_test/big_tuples/to_test"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) Roundtrip(ctx context.Context, a []*to_test.T17) ([]*to_test.T17, error) {
	return a, nil
}

func (handler) NestedFuture(ctx context.Context, a *struct {
	V0  uint8
	V1  uint8
	V2  uint8
	V3  uint8
	V4  uint8
	V5  uint8
	V6  uint8
	V7  uint8
	V8  uint8
	V9  uint8
	V10 uint8
	V11 uint8
	V12 uint8
	V13 uint8
	V14 uint8
	V15 uint8
	V16 <-chan uint8
}) (<-chan uint8, error) {
	return a.V16, nil
}

func (handler) ManyParams(ctx context.Context, a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13, a14, a15 uint8, a16 string) (string, error) {
	if [16]uint8{a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13, a14, a15} != [16]uint8{0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15} {
		return "", fmt.Errorf("unexpected params")
	}
	return a16, nil
}
