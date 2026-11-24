// Package wrpc implements wRPC for Go.
//
// This package is not ready for production use.
package wrpc

import "io"

// Own is an owned resource handle
type Own[T any] []byte

// Borrow returns the handle as a [`Borrow`]
func (v Own[T]) Borrow() Borrow[T] {
	return Borrow[T](v)
}

// Borrow is a borrowed resource handle
type Borrow[T any] []byte

// ByteReader is the reader value codecs decode from
type ByteReader interface {
	io.ByteReader
	io.Reader
}
