package test

import (
	"context"
	"runtime"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) F(ctx context.Context) error {
	for range 10 {
		runtime.Gosched()
	}
	return nil
}
