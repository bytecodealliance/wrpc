package test

import (
	"context"
	"sync"
)

var (
	mu   sync.Mutex
	slot <-chan struct{}
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) Set(ctx context.Context, future <-chan struct{}) error {
	mu.Lock()
	defer mu.Unlock()
	slot = future
	return nil
}

func (handler) Get(ctx context.Context) (<-chan struct{}, error) {
	mu.Lock()
	defer mu.Unlock()
	future := slot
	slot = nil
	return future, nil
}
