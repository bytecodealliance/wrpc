package test

import "context"

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) TakeThenDrop(ctx context.Context, x <-chan string) error {
	return nil
}

func (handler) ReadAndDrop(ctx context.Context, x <-chan string) error {
	<-x
	return nil
}
