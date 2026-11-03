package test

import (
	"context"
	"fmt"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) CancelBeforeRead(ctx context.Context, x <-chan uint32) error {
	return nil
}

func (handler) CancelAfterRead(ctx context.Context, x <-chan uint32) error {
	select {
	case <-x:
	default:
	}
	return nil
}

func (handler) StartReadThenCancel(ctx context.Context, data <-chan uint32, signal <-chan struct{}) error {
	select {
	case v := <-data:
		assertEqual(v, 4)
		return nil
	default:
	}
	<-signal
	assertEqual(<-data, 4)
	return nil
}

func assertEqual[T comparable](a T, b T) {
	if a != b {
		panic(fmt.Sprintf("%v not equal to %v", a, b))
	}
}
