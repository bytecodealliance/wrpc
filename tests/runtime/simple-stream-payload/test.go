package test

import (
	"context"
	"fmt"
	"slices"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) ReadStream(ctx context.Context, x <-chan []uint8) error {
	var buf []uint8
	take := func(n int) []uint8 {
		for len(buf) < n {
			chunk, ok := <-x
			if !ok {
				break
			}
			buf = append(buf, chunk...)
		}
		n = min(n, len(buf))
		items := buf[:n]
		buf = buf[n:]
		return items
	}
	// read one item
	assertEqual(take(1), []uint8{0})
	// read two items
	assertEqual(take(2), []uint8{1, 2})
	// read 1/2 items
	assertEqual(take(1), []uint8{3})
	// read the next buffered item
	assertEqual(take(1), []uint8{4})
	return nil
}

func assertEqual(a []uint8, b []uint8) {
	if !slices.Equal(a, b) {
		panic(fmt.Sprintf("%v not equal to %v", a, b))
	}
}
