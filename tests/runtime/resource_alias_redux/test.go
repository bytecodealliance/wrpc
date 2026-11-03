package test

import (
	"context"

	wrpc "wrpc.io/go"

	a1 "driver/test/exports/test/resource_alias_redux/resource_alias1"
	a2 "driver/test/exports/test/resource_alias_redux/resource_alias2"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) A(ctx context.Context, f *a1.Foo) ([]wrpc.Own[a1.Thing], error) {
	return []wrpc.Own[a1.Thing]{f.Thing}, nil
}

func (handler) Thing(ctx context.Context, s string) (wrpc.Own[a1.Thing], error) {
	return wrpc.Own[a1.Thing](s + " GuestThing"), nil
}

func (handler) Thing_Get(ctx context.Context, self wrpc.Borrow[a1.Thing]) (string, error) {
	return string(self) + " GuestThing.get", nil
}

func (handler) B(ctx context.Context, f *a2.Foo, g *a2.Bar) ([]wrpc.Own[a2.Thing], error) {
	return []wrpc.Own[a2.Thing]{f.Thing, g.Thing}, nil
}

func (handler) Test(ctx context.Context, things []wrpc.Own[a1.Thing]) ([]wrpc.Own[a1.Thing], error) {
	return things, nil
}
