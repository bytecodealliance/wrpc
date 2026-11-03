package test

import (
	"context"
	"fmt"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) ReadStream(ctx context.Context, x <-chan []struct{}) error {
	var buffered int
	take := func(n int) int {
		for buffered < n {
			chunk, ok := <-x
			if !ok {
				break
			}
			buffered += len(chunk)
		}
		n = min(n, buffered)
		buffered -= n
		return n
	}
	assertEqual(take(1), 1)
	assertEqual(take(2), 2)
	return nil
}

func assertEqual[T comparable](a T, b T) {
	if a != b {
		panic(fmt.Sprintf("%v not equal to %v", a, b))
	}
}
