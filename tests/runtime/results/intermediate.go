package intermediate

import (
	"context"

	wrpc "wrpc.io/go"

	exports "driver/intermediate/exports/test/results/test"
	imports "driver/intermediate/test/results/test"
)

type handler struct {
	c wrpc.Invoker
}

func NewHandler(c wrpc.Invoker) handler {
	return handler{c}
}

func (h handler) StringError(ctx context.Context, a float32) (*wrpc.Result[float32, string], error) {
	return imports.StringError(ctx, h.c, a)
}

func (h handler) EnumError(ctx context.Context, a float32) (*wrpc.Result[float32, exports.E], error) {
	res, err := imports.EnumError(ctx, h.c, a)
	if err != nil {
		return nil, err
	}
	if res.Err != nil {
		return wrpc.Err[float32](exports.E(*res.Err)), nil
	}
	return wrpc.Ok[exports.E](*res.Ok), nil
}

func (h handler) RecordError(ctx context.Context, a float32) (*wrpc.Result[float32, exports.E2], error) {
	res, err := imports.RecordError(ctx, h.c, a)
	if err != nil {
		return nil, err
	}
	if res.Err != nil {
		return wrpc.Err[float32](exports.E2{Line: res.Err.Line, Column: res.Err.Column}), nil
	}
	return wrpc.Ok[exports.E2](*res.Ok), nil
}

func (h handler) VariantError(ctx context.Context, a float32) (*wrpc.Result[float32, exports.E3], error) {
	res, err := imports.VariantError(ctx, h.c, a)
	if err != nil {
		return nil, err
	}
	if res.Err == nil {
		return wrpc.Ok[exports.E3](*res.Ok), nil
	}
	if e, ok := res.Err.GetE1(); ok {
		return wrpc.Err[float32](*exports.NewE3E1(exports.E(e))), nil
	}
	e, _ := res.Err.GetE2()
	return wrpc.Err[float32](*exports.NewE3E2(&exports.E2{Line: e.Line, Column: e.Column})), nil
}

func (h handler) EmptyError(ctx context.Context, a uint32) (*wrpc.Result[uint32, struct{}], error) {
	return imports.EmptyError(ctx, h.c, a)
}

func (h handler) DoubleError(ctx context.Context, a uint32) (*wrpc.Result[wrpc.Result[struct{}, string], string], error) {
	return imports.DoubleError(ctx, h.c, a)
}
