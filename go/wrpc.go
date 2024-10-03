package wrpc

import (
	"context"
	"io"
)

// Invoke is the client-side transport handle
type Invoker interface {
	// Invoke invokes a function `name` within an instance `instance`.
	// Initial, encoded payload must be specified in `b`.
	// `paths` define the async result paths to subscribe on.
	// On success, `Invoke` returns two handles used for writing and reading encoded parameters and results respectively.
	// NOTE: if the returned handle is used for writing, `b` must be non-empty.
	Invoke(ctx context.Context, instance string, name string, b []byte, paths ...SubscribePath) (IndexWriteCloser, IndexReadCloser, error)
}

// Server is the server-side transport handle
type Server interface {
	// Serve serves a function `name` within an instance `instance`.
	// `paths` define the async parameter paths to subscribe on.
	// `Serve` will call `f` with two handles used for writing and reading encoded results and parameters respectively.
	// On success, `Serve` returns a function, which can be called to stop serving.
	Serve(instance string, name string, f func(context.Context, IndexWriteCloser, IndexReadCloser), paths ...SubscribePath) (func() error, error)
}

// Own is an owned resource handle
type Own[T any] []byte

// Borrow returns the handle as a [`Borrow`]
func (v Own[T]) Borrow() Borrow[T] {
	return Borrow[T](v)
}

// Borrow is a borrowed resource handle
type Borrow[T any] []byte

// SubscribePath is the subscription path.
// `nil` represents a wildcard index used for dynamically-sized values, like `list`
type SubscribePath []*uint32

// NewSubscribePath creates a new subscription path.
func NewSubscribePath(ps ...*uint32) SubscribePath {
	return SubscribePath(ps)
}

func (p SubscribePath) push(v *uint32) SubscribePath {
	return SubscribePath(append(append(make(SubscribePath, 0, len(p)+1), p...), v))
}

// Index pushes a `uint32` index to the path
func (p SubscribePath) Index(i uint32) SubscribePath {
	return p.push(&i)
}

// Wildcard pushes a wildcard index to the path used for dynamically-sized values, like `list`
func (p SubscribePath) Wildcard() SubscribePath {
	return p.push(nil)
}

// Parent pops the last element in the path and returns the resulting path and `true` and success.
// It returns `nil`, `false` otherwise.
func (p SubscribePath) Parent() (SubscribePath, bool) {
	n := len(p)
	if n == 0 {
		return nil, false
	}
	return SubscribePath(p[:n-1]), true
}

// Index represents entities, which can be indexed by concrete `uint32` paths, for example transport streams.
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
