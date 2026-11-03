//@ wasmtime-flags = '-Wcomponent-model-fixed-length-lists'

package test

import (
	"context"
	"fmt"

	"driver/test/exports/test/fixed_length_lists/to_test"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) ListParam(ctx context.Context, a [4]uint32) error {
	assertEqual(a, [4]uint32{1, 2, 3, 4})
	return nil
}

func (handler) ListParam2(ctx context.Context, a [2][2]uint32) error {
	assertEqual(a, [2][2]uint32{{1, 2}, {3, 4}})
	return nil
}

func (handler) ListParam3(ctx context.Context, a [20]int32) error {
	assertEqual(a, [20]int32{-1, 2, -3, 4, -5, 6, -7, 8, -9, 10, -11, 12, -13, 14, -15, 16, -17, 18, -19, 20})
	return nil
}

func (handler) ListMinmax16(ctx context.Context, a [4]uint16, b [4]int16) ([4]uint16, [4]int16, error) {
	return a, b, nil
}

func (handler) ListMinmaxFloat(ctx context.Context, a [2]float32, b [2]float64) ([2]float32, [2]float64, error) {
	return a, b, nil
}

func (handler) ListRoundtrip(ctx context.Context, a [12]uint8) ([12]uint8, error) {
	return a, nil
}

func (handler) ListResult(ctx context.Context) ([8]uint8, error) {
	return [8]uint8{'0', '1', 'A', 'B', 'a', 'b', 128, 255}, nil
}

func (handler) NestedRoundtrip(ctx context.Context, a [2][2]uint32, b [2][2]int32) ([2][2]uint32, [2][2]int32, error) {
	return a, b, nil
}

func (handler) LargeRoundtrip(ctx context.Context, a [2][2]uint32, b [4][4]int32) ([2][2]uint32, [4][4]int32, error) {
	return a, b, nil
}

func (handler) NightmareOnCpp(ctx context.Context, a [2]*to_test.Nested) ([2]*to_test.Nested, error) {
	return a, nil
}

func assertEqual[T comparable](a T, b T) {
	if a != b {
		panic(fmt.Sprintf("%v not equal to %v", a, b))
	}
}
