package test

import (
	"context"
	"encoding/binary"

	wrpc "wrpc.io/go"

	"driver/test/exports/test/resource_alias/e1"
	"driver/test/exports/test/resource_alias/e2"
)

type e1Handler struct{}

func (e1Handler) A(ctx context.Context, f *e1.Foo) ([]wrpc.Own[e1.X], error) {
	return []wrpc.Own[e1.X]{f.X}, nil
}

func (e1Handler) X(ctx context.Context, v uint32) (wrpc.Own[e1.X], error) {
	return binary.LittleEndian.AppendUint32(nil, v), nil
}

type e2Handler struct{}

func (e2Handler) A(ctx context.Context, f *e2.Foo, g *e2.Bar, h wrpc.Borrow[e2.Y]) ([]wrpc.Own[e2.Y], error) {
	return []wrpc.Own[e2.Y]{f.X, g.X}, nil
}

func NewHandler() (e1Handler, e2Handler) {
	return e1Handler{}, e2Handler{}
}
