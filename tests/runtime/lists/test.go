package test

import (
	"context"
	"slices"

	wrpc "wrpc.io/go"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) AllocatedBytes(ctx context.Context) (uint32, error) {
	return 0, nil
}

func (handler) EmptyListParam(ctx context.Context, x []uint8) error {
	if len(x) != 0 {
		panic("trouble")
	}
	return nil
}

func (handler) EmptyStringParam(ctx context.Context, x string) error {
	if len(x) != 0 {
		panic("trouble")
	}
	return nil
}

func (handler) EmptyListResult(ctx context.Context) ([]uint8, error) {
	return []uint8{}, nil
}

func (handler) EmptyStringResult(ctx context.Context) (string, error) {
	return "", nil
}

func (handler) ListParam(ctx context.Context, x []uint8) error {
	if !slices.Equal(x, []uint8{1, 2, 3, 4}) {
		panic("trouble")
	}
	return nil
}

func (handler) ListParam2(ctx context.Context, x string) error {
	if x != "foo" {
		panic("trouble")
	}
	return nil
}

func (handler) ListParam3(ctx context.Context, x []string) error {
	if !slices.Equal(x, []string{"foo", "bar", "baz"}) {
		panic("trouble")
	}
	return nil
}

func (handler) ListParam4(ctx context.Context, x [][]string) error {
	if !slices.Equal(x[0], []string{"foo", "bar"}) {
		panic("trouble")
	}
	if !slices.Equal(x[1], []string{"baz"}) {
		panic("trouble")
	}
	return nil
}

func (handler) ListParam5(ctx context.Context, x []*wrpc.Tuple3[uint8, uint32, uint8]) error {
	expected := []wrpc.Tuple3[uint8, uint32, uint8]{
		{V0: 1, V1: 2, V2: 3},
		{V0: 4, V1: 5, V2: 6},
	}
	if len(x) != len(expected) {
		panic("trouble")
	}
	for i, v := range x {
		if *v != expected[i] {
			panic("trouble")
		}
	}
	return nil
}

func (handler) ListParamLarge(ctx context.Context, x []string) error {
	if len(x) != 1000 {
		panic("trouble")
	}
	return nil
}

func (handler) ListResult(ctx context.Context) ([]uint8, error) {
	return []uint8{1, 2, 3, 4, 5}, nil
}

func (handler) ListResult2(ctx context.Context) (string, error) {
	return "hello!", nil
}

func (handler) ListResult3(ctx context.Context) ([]string, error) {
	return []string{"hello,", "world!"}, nil
}

func (handler) ListRoundtrip(ctx context.Context, x []uint8) ([]uint8, error) {
	return x, nil
}

func (handler) StringRoundtrip(ctx context.Context, x string) (string, error) {
	return x, nil
}

func (handler) ListMinmax8(ctx context.Context, x []uint8, y []int8) ([]uint8, []int8, error) {
	return x, y, nil
}

func (handler) ListMinmax16(ctx context.Context, x []uint16, y []int16) ([]uint16, []int16, error) {
	return x, y, nil
}

func (handler) ListMinmax32(ctx context.Context, x []uint32, y []int32) ([]uint32, []int32, error) {
	return x, y, nil
}

func (handler) ListMinmax64(ctx context.Context, x []uint64, y []int64) ([]uint64, []int64, error) {
	return x, y, nil
}

func (handler) ListMinmaxFloat(ctx context.Context, x []float32, y []float64) ([]float32, []float64, error) {
	return x, y, nil
}

func (handler) WasiHttpHeadersRoundtrip(ctx context.Context, x []*wrpc.Tuple2[string, []uint8]) ([]*wrpc.Tuple2[string, []uint8], error) {
	return x, nil
}
