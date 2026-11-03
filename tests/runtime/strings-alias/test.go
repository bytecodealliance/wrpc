package test

import (
	"context"
	"fmt"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) Foo(ctx context.Context, x string) error {
	if x != "hello" {
		panic(fmt.Sprintf("unexpected value: `%v`", x))
	}
	return nil
}

func (handler) Bar(ctx context.Context) (string, error) {
	return "world", nil
}
