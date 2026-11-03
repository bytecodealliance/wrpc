package resource_with_lists

import (
	"context"

	wrpc "wrpc.io/go"

	export "driver/resource_with_lists/exports/test/resource_with_lists/test"
	"driver/resource_with_lists/test/resource_with_lists/test"
)

type handler struct {
	c wrpc.Invoker
}

func NewHandler(c wrpc.Invoker) handler {
	return handler{c}
}

func (h handler) Thing(ctx context.Context, l []byte) (wrpc.Own[export.Thing], error) {
	result, err := test.NewThing(ctx, h.c, append(l, " Thing"...))
	if err != nil {
		return nil, err
	}
	return wrpc.Own[export.Thing](result), nil
}

func (h handler) Thing_Foo(ctx context.Context, self wrpc.Borrow[export.Thing]) ([]byte, error) {
	list, err := test.Thing_Foo(ctx, h.c, wrpc.Borrow[test.Thing](self))
	if err != nil {
		return nil, err
	}
	return append(list, " Thing.foo"...), nil
}

func (h handler) Thing_Bar(ctx context.Context, self wrpc.Borrow[export.Thing], l []byte) error {
	return test.Thing_Bar(ctx, h.c, wrpc.Borrow[test.Thing](self), append(l, " Thing.bar"...))
}

func (h handler) Thing_Baz(ctx context.Context, l []byte) ([]byte, error) {
	list2, err := test.Thing_Baz(ctx, h.c, append(l, " Thing.baz"...))
	if err != nil {
		return nil, err
	}
	return append(list2, " Thing.baz again"...), nil
}
