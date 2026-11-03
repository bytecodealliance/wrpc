package test

import "context"

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) X(ctx context.Context) error {
	return nil
}
