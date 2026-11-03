package test

import "context"

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) ReturnString(ctx context.Context) (string, error) {
	return "hello", nil
}
