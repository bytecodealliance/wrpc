package leaf

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/leaf/exports/test/results/test"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) StringError(ctx context.Context, a float32) (*wrpc.Result[float32, string], error) {
	if a == 0.0 {
		return wrpc.Err[float32]("zero"), nil
	}
	return wrpc.Ok[string](a), nil
}

func (handler) EnumError(ctx context.Context, a float32) (*wrpc.Result[float32, test.E], error) {
	if a == 0.0 {
		return wrpc.Err[float32](test.E_A), nil
	}
	return wrpc.Ok[test.E](a), nil
}

func (handler) RecordError(ctx context.Context, a float32) (*wrpc.Result[float32, test.E2], error) {
	switch a {
	case 0.0:
		return wrpc.Err[float32](test.E2{Line: 420, Column: 0}), nil
	case 1.0:
		return wrpc.Err[float32](test.E2{Line: 77, Column: 2}), nil
	default:
		return wrpc.Ok[test.E2](a), nil
	}
}

func (handler) VariantError(ctx context.Context, a float32) (*wrpc.Result[float32, test.E3], error) {
	switch a {
	case 0.0:
		return wrpc.Err[float32](*test.NewE3E2(&test.E2{Line: 420, Column: 0})), nil
	case 1.0:
		return wrpc.Err[float32](*test.NewE3E1(test.E_B)), nil
	case 2.0:
		return wrpc.Err[float32](*test.NewE3E1(test.E_C)), nil
	default:
		return wrpc.Ok[test.E3](a), nil
	}
}

func (handler) EmptyError(ctx context.Context, a uint32) (*wrpc.Result[uint32, struct{}], error) {
	switch a {
	case 0:
		return wrpc.Err[uint32](struct{}{}), nil
	case 1:
		return wrpc.Ok[struct{}](uint32(42)), nil
	default:
		return wrpc.Ok[struct{}](a), nil
	}
}

func (handler) DoubleError(ctx context.Context, a uint32) (*wrpc.Result[wrpc.Result[struct{}, string], string], error) {
	switch a {
	case 0:
		return wrpc.Ok[string](*wrpc.Ok[string](struct{}{})), nil
	case 1:
		return wrpc.Ok[string](*wrpc.Err[struct{}]("one")), nil
	default:
		return wrpc.Err[wrpc.Result[struct{}, string]]("two"), nil
	}
}
