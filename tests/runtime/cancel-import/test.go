package test

import "context"

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) PendingImport(ctx context.Context, x <-chan struct{}) error {
	<-x
	return nil
}

func (handler) BackpressureSet(ctx context.Context, x bool) error {
	return nil
}
