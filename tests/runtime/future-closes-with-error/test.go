package test

import "context"

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) F(ctx context.Context, future <-chan struct{}) error {
	<-future
	return nil
}
