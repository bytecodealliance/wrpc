package middle

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/middle/test/common/test_types"
	"driver/middle/test/common/to_test"
)

type handler struct {
	connect func() (*wrpc.Framed, error)
}

func NewHandler(connect func() (*wrpc.Framed, error)) handler {
	return handler{connect}
}

func (h handler) conn() *wrpc.Framed {
	c, err := h.connect()
	if err != nil {
		panic(err)
	}
	return c
}

func (h handler) Wrap(ctx context.Context, flag *test_types.F1) (*test_types.R1, error) {
	return to_test.Wrap(ctx, h.conn(), flag)
}

func (h handler) VarF(ctx context.Context) (*test_types.V1, error) {
	return to_test.VarF(ctx, h.conn())
}
