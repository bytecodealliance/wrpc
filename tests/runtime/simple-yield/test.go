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
	runtime.Gosched()
	return nil
}
