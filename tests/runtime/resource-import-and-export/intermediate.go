package intermediate

import (
	"context"

	wrpc "wrpc.io/go"

	export "driver/intermediate/exports/test/resource_import_and_export/test"
	"driver/intermediate/test/resource_import_and_export/test"
)

type handler struct {
	c wrpc.Invoker
}

func NewHandler(c wrpc.Invoker) handler {
	return handler{c}
}

func (h handler) ToplevelExport(ctx context.Context, input wrpc.Own[Thing]) (wrpc.Own[Thing], error) {
	return ToplevelImport(ctx, h.c, input)
}

func (h handler) Thing(ctx context.Context, v uint32) (wrpc.Own[export.Thing], error) {
	thing, err := test.NewThing(ctx, h.c, v+1)
	if err != nil {
		return nil, err
	}
	return wrpc.Own[export.Thing](thing), nil
}

func (h handler) Thing_Foo(ctx context.Context, self wrpc.Borrow[export.Thing]) (uint32, error) {
	v, err := test.Thing_Foo(ctx, h.c, wrpc.Borrow[test.Thing](self))
	if err != nil {
		return 0, err
	}
	return v + 2, nil
}

func (h handler) Thing_Bar(ctx context.Context, self wrpc.Borrow[export.Thing], v uint32) error {
	return test.Thing_Bar(ctx, h.c, wrpc.Borrow[test.Thing](self), v+3)
}

func (h handler) Thing_Baz(ctx context.Context, a wrpc.Own[export.Thing], b wrpc.Own[export.Thing]) (wrpc.Own[export.Thing], error) {
	thing, err := test.Thing_Baz(ctx, h.c, wrpc.Own[test.Thing](a), wrpc.Own[test.Thing](b))
	if err != nil {
		return nil, err
	}
	result, err := test.Thing_Foo(ctx, h.c, thing.Borrow())
	if err != nil {
		return nil, err
	}
	return h.Thing(ctx, result+4)
}
