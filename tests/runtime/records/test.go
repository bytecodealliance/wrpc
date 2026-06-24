package test

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/test/exports/test/records/to_test"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) MultipleResults(ctx context.Context) (uint8, uint16, error) {
	return 4, 5, nil
}

func (handler) SwapTuple(ctx context.Context, a *wrpc.Tuple2[uint8, uint32]) (uint32, uint8, error) {
	return a.V1, a.V0, nil
}

func (handler) RoundtripFlags1(ctx context.Context, a *to_test.F1) (*to_test.F1, error) {
	return a, nil
}

func (handler) RoundtripFlags2(ctx context.Context, a *to_test.F2) (*to_test.F2, error) {
	return a, nil
}

func (handler) RoundtripFlags3(ctx context.Context, a *to_test.Flag8, b *to_test.Flag16, c *to_test.Flag32) (*to_test.Flag8, *to_test.Flag16, *to_test.Flag32, error) {
	return a, b, c, nil
}

func (handler) RoundtripRecord1(ctx context.Context, a *to_test.R1) (*to_test.R1, error) {
	return a, nil
}

func (handler) Tuple1(ctx context.Context, a uint8) (uint8, error) {
	return a, nil
}
