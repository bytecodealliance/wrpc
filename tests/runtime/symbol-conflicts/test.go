package test

import "context"

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) Foo(ctx context.Context) error {
	return nil
}

func (handler) Bar(ctx context.Context) (string, error) {
	return "", nil
}
