package test

import (
	"context"
	"fmt"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) Foo(ctx context.Context, x []byte) error {
	if string(x) != "hello" {
		panic(fmt.Sprintf("unexpected value: `%s`", x))
	}
	return nil
}

func (handler) Bar(ctx context.Context) ([]byte, error) {
	return []byte("world"), nil
}
