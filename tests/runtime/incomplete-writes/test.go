package test

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/test/exports/my/test/test_interface"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) TestThing(ctx context.Context, s string) (wrpc.Own[test_interface.TestThing], error) {
	return wrpc.Own[test_interface.TestThing](s), nil
}

func (handler) TestThing_Get(ctx context.Context, self wrpc.Borrow[test_interface.TestThing]) (string, error) {
	return string(self), nil
}

func (handler) ShortReadsTest(ctx context.Context, stream <-chan []wrpc.Own[test_interface.TestThing]) (<-chan []wrpc.Own[test_interface.TestThing], error) {
	return shortReads(stream), nil
}

func (handler) ShortReadsLeaf(ctx context.Context, stream <-chan []wrpc.Own[test_interface.LeafThing]) (<-chan []wrpc.Own[test_interface.LeafThing], error) {
	return shortReads(stream), nil
}

func (handler) DroppedReaderTest(ctx context.Context, f1 <-chan wrpc.Own[test_interface.TestThing], f2 <-chan wrpc.Own[test_interface.TestThing]) (<-chan wrpc.Own[test_interface.TestThing], <-chan wrpc.Own[test_interface.TestThing], error) {
	rx1, rx2 := droppedReader(f2)
	return rx1, rx2, nil
}

func (handler) DroppedReaderLeaf(ctx context.Context, f1 <-chan wrpc.Own[test_interface.LeafThing], f2 <-chan wrpc.Own[test_interface.LeafThing]) (<-chan wrpc.Own[test_interface.LeafThing], <-chan wrpc.Own[test_interface.LeafThing], error) {
	rx1, rx2 := droppedReader(f2)
	return rx1, rx2, nil
}

func shortReads[T any](stream <-chan []T) <-chan []T {
	tx := make(chan []T, 1)
	go func() {
		defer close(tx)

		var things []T
		for chunk := range stream {
			things = append(things, chunk...)
		}

		// Write the things all at once.  The caller will read them only
		// one at a time.
		tx <- things
	}()
	return tx
}

func droppedReader[T any](f2 <-chan T) (<-chan T, <-chan T) {
	tx1 := make(chan T, 1)
	tx2 := make(chan T, 1)
	go func() {
		// Drop the first future without reading from it.
		thing := <-f2

		// Write the thing to the first future, the read end of which
		// the caller will drop without reading from, then write it again
		// to the second future, which the caller will read.
		tx1 <- thing
		close(tx1)
		tx2 <- thing
		close(tx2)
	}()
	return tx1, tx2
}
