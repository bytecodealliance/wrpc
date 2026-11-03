package test

import (
	"context"
	"fmt"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) F(ctx context.Context, future <-chan uint8) error {
	if v := <-future; v != 0 {
		panic(fmt.Sprintf("%v not equal to 0", v))
	}
	return nil
}
