package middle

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/middle/test/common/test_types"
	"driver/middle/test/common/to_test"
)

type handler struct {
	c wrpc.Invoker
}

func NewHandler(c wrpc.Invoker) handler {
	return handler{c}
}

func (h handler) Wrap(ctx context.Context, flag *test_types.F1) (*test_types.R1, error) {
	return to_test.Wrap(ctx, h.c, flag)
}

func (h handler) VarF(ctx context.Context) (*test_types.V1, error) {
	return to_test.VarF(ctx, h.c)
}
