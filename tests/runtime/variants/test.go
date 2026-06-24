package test

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/test/exports/test/variants/to_test"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) RoundtripOption(ctx context.Context, a *float32) (*uint8, error) {
	if a == nil {
		return nil, nil
	}
	v := uint8(*a)
	return &v, nil
}

func (handler) RoundtripResult(ctx context.Context, a *wrpc.Result[uint32, float32]) (*wrpc.Result[float64, uint8], error) {
	if a.Ok != nil {
		return wrpc.Ok[uint8, float64](float64(*a.Ok)), nil
	}
	return wrpc.Err[float64, uint8](uint8(*a.Err)), nil
}

func (handler) RoundtripEnum(ctx context.Context, a to_test.E1) (to_test.E1, error) {
	return a, nil
}

func (handler) InvertBool(ctx context.Context, a bool) (bool, error) {
	return !a, nil
}

func (handler) VariantCasts(ctx context.Context, a *to_test.Casts) (*to_test.C1, *to_test.C2, *to_test.C3, *to_test.C4, *to_test.C5, *to_test.C6, error) {
	return a.V0, a.V1, a.V2, a.V3, a.V4, a.V5, nil
}

func (handler) VariantZeros(ctx context.Context, a *to_test.Zeros) (*to_test.Z1, *to_test.Z2, *to_test.Z3, *to_test.Z4, error) {
	return a.V0, a.V1, a.V2, a.V3, nil
}

func (handler) VariantTypedefs(ctx context.Context, a *uint32, b bool, c *to_test.ResultTypedef) error {
	return nil
}

func (handler) VariantEnums(ctx context.Context, a bool, b *wrpc.Result[struct{}, struct{}], c to_test.MyErrno) (bool, *wrpc.Result[struct{}, struct{}], to_test.MyErrno, error) {
	return a, b, c, nil
}
