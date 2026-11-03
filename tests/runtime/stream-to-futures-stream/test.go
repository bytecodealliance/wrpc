package test

import (
	"context"
	"fmt"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) ReadStream(ctx context.Context, x <-chan []uint8) error {
	// Flatten the chunked stream into a sequence of items
	var items []uint8
	next := func() (uint8, bool) {
		for len(items) == 0 {
			chunk, ok := <-x
			if !ok {
				return 0, false
			}
			items = chunk
		}
		v := items[0]
		items = items[1:]
		return v, true
	}
	// Read all items one by one
	assertNext(next, 10)
	assertNext(next, 20)
	assertNext(next, 30)
	// Stream should be exhausted after the writer is closed
	if v, ok := next(); ok {
		panic(fmt.Sprintf("expected end of stream, got %v", v))
	}
	return nil
}

func assertNext(next func() (uint8, bool), expected uint8) {
	v, ok := next()
	if !ok || v != expected {
		panic(fmt.Sprintf("expected %v, got %v (ok: %v)", expected, v, ok))
	}
}
