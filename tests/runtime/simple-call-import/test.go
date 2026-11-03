package test

import "context"

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) F(ctx context.Context) error {
	return nil
}
