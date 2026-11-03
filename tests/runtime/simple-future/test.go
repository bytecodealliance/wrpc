package test

import "context"

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) ReadFuture(ctx context.Context, x <-chan struct{}) error {
	<-x
	return nil
}

func (handler) DropFuture(ctx context.Context, x <-chan struct{}) error {
	return nil
}
