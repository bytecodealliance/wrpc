package test

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/test/exports/test/resource_borrow_in_record/to_test"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) Test(ctx context.Context, a []*to_test.Foo) ([]wrpc.Own[to_test.Thing], error) {
	things := make([]wrpc.Own[to_test.Thing], 0, len(a))
	for _, foo := range a {
		things = append(things, wrpc.Own[to_test.Thing](string(foo.Thing)+" test"))
	}
	return things, nil
}

func (handler) Thing(ctx context.Context, s string) (wrpc.Own[to_test.Thing], error) {
	return wrpc.Own[to_test.Thing](s + " new"), nil
}

func (handler) Thing_Get(ctx context.Context, self wrpc.Borrow[to_test.Thing]) (string, error) {
	return string(self) + " get", nil
}
