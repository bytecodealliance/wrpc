package intermediate

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/intermediate/exports/exports_"
	"driver/intermediate/imports"
	"driver/intermediate/test/resource_floats/test"
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

func (h handler) Add(ctx context.Context, a wrpc.Borrow[Float], b wrpc.Borrow[Float]) (wrpc.Own[Float], error) {
	av, err := test.Float_Get(ctx, h.conn(), a)
	if err != nil {
		return nil, err
	}
	bv, err := test.Float_Get(ctx, h.conn(), b)
	if err != nil {
		return nil, err
	}
	return test.NewFloat(ctx, h.conn(), av+bv+5.0)
}

func (h handler) Float(ctx context.Context, v float64) (wrpc.Own[exports_.Float], error) {
	val, err := imports.NewFloat(ctx, h.conn(), v+1.0)
	if err != nil {
		return nil, err
	}
	return wrpc.Own[exports_.Float](val), nil
}

func (h handler) Float_Get(ctx context.Context, self wrpc.Borrow[exports_.Float]) (float64, error) {
	v, err := imports.Float_Get(ctx, h.conn(), wrpc.Borrow[imports.Float](self))
	if err != nil {
		return 0, err
	}
	return v + 3.0, nil
}

func (h handler) Float_Add(ctx context.Context, a wrpc.Own[exports_.Float], b float64) (wrpc.Own[exports_.Float], error) {
	res, err := imports.Float_Add(ctx, h.conn(), wrpc.Own[imports.Float](a), b)
	if err != nil {
		return nil, err
	}
	v, err := imports.Float_Get(ctx, h.conn(), res.Borrow())
	if err != nil {
		return nil, err
	}
	return h.Float(ctx, v+5.0)
}
