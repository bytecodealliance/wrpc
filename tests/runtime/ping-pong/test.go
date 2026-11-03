package test

import "context"

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) Ping(ctx context.Context, x <-chan string, y string) (<-chan string, error) {
	msg := <-x + y
	rx := make(chan string, 1)
	rx <- msg
	close(rx)
	return rx, nil
}

func (handler) Pong(ctx context.Context, x <-chan string) (string, error) {
	return <-x, nil
}
