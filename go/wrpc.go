package wrpc

import (
	"context"
	"io"
)

type Invoker interface {
	Invoke(ctx context.Context, instance string, name string, b []byte, paths ...SubscribePath) (IndexWriteCloser, IndexReadCloser, error)
}

type Server interface {
	Serve(instance string, name string, f func(context.Context, IndexWriteCloser, IndexReadCloser), paths ...SubscribePath) (func() error, error)
}

// Own is an owned resource handle
type Own[T any] []byte

func (v Own[T]) Borrow() Borrow[T] {
	return Borrow[T](v)
}

// Borrow is a borrowed resource handle
type Borrow[T any] []byte

type SubscribePath []*uint32

func NewSubscribePath(ps ...*uint32) SubscribePath {
	return SubscribePath(ps)
}

func (p SubscribePath) push(v *uint32) SubscribePath {
	return SubscribePath(append(append(make(SubscribePath, 0, len(p)+1), p...), v))
}

func (p SubscribePath) Index(i uint32) SubscribePath {
	return p.push(&i)
}

func (p SubscribePath) Wildcard() SubscribePath {
	return p.push(nil)
}

func (p SubscribePath) Parent() (SubscribePath, bool) {
	n := len(p)
	if n == 0 {
		return nil, false
	}
	return SubscribePath(p[:n-1]), true
}

type Index[T any] interface {
	Index(path ...uint32) (T, error)
}

type IndexReader interface {
	io.Reader
	io.ByteReader

	Index[IndexReadCloser]
}

type IndexWriter interface {
	io.Writer
	io.ByteWriter

	Index[IndexWriteCloser]
}

type IndexReadCloser interface {
	IndexReader
	io.Closer
}

type IndexWriteCloser interface {
	IndexWriter
	io.Closer
}

type ByteWriter interface {
	io.ByteWriter
	io.Writer
}

type ByteReader interface {
	io.ByteReader
	io.Reader
}

type Receiver[T any] interface {
	Receive() (T, error)
	io.Closer
}

type ByteReadCloser interface {
	ByteReader
	io.Closer
}

type CompleteReceiver[T any] struct {
	v     T
	ready bool
}

func (r *CompleteReceiver[T]) Receive() (T, error) {
	defer func() {
		r.ready = false
	}()
	if !r.ready {
		return CompleteReceiver[T]{}.v, io.EOF
	}
	return r.v, nil
}

func (r *CompleteReceiver[T]) Close() error {
	if c, ok := (any)(r.v).(io.Closer); ok {
		return c.Close()
	}
	return nil
}

func NewCompleteReceiver[T any](v T) *CompleteReceiver[T] {
	return &CompleteReceiver[T]{v, true}
}

type DecodeReceiver[T any] struct {
	r      IndexReadCloser
	decode func(IndexReadCloser) (T, error)
}

func (r *DecodeReceiver[T]) Receive() (T, error) {
	return r.decode(r.r)
}

func (r *DecodeReceiver[T]) Close() error {
	return r.r.Close()
}

func NewDecodeReceiver[T any](r IndexReadCloser, decode func(IndexReadCloser) (T, error)) *DecodeReceiver[T] {
	return &DecodeReceiver[T]{r, decode}
}

type nestedReceiver[T any, A Receiver[B], B Receiver[T]] struct {
	rx A
}

func (r nestedReceiver[T, A, B]) Receive() (Receiver[T], error) {
	return r.rx.Receive()
}

func (r nestedReceiver[T, A, B]) Close() error {
	return r.rx.Close()
}

func NewNestedReceiver[T any, A Receiver[B], B Receiver[T]](rx A) Receiver[Receiver[T]] {
	return nestedReceiver[T, A, B]{rx}
}
