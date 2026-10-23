package wrpc

import (
	"bufio"
	"context"
	"fmt"
	"io"
	"slices"
)

// Invoke is the client-side transport handle
type Invoker interface {
	// Invoke invokes a function `name` within an instance `instance`.
	// Initial, encoded payload must be specified in `b`.
	// `paths` define the async result paths to subscribe on.
	// On success, `Invoke` returns two handles used for writing and reading encoded parameters and results respectively.
	Invoke(ctx context.Context, instance string, name string, b []byte, paths ...SubscribePath) (IndexWriteCloser, IndexReadCloser, error)
}

type HandleFunc func(context.Context, IndexWriteCloser, IndexReadCloser)

// Server is the server-side transport handle
type Server interface {
	// Serve serves a function `name` within an instance `instance`.
	// `paths` define the async parameter paths to subscribe on.
	// `Serve` will call `f` with two handles used for writing and reading encoded results and parameters respectively.
	// On success, `Serve` returns a function, which can be called to stop serving.
	Serve(instance string, name string, f HandleFunc, paths ...SubscribePath) (func() error, error)
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

	// Buffered returns the number of bytes, which were received, but not read yet
	Buffered() int
}

type IndexWriter interface {
	io.Writer
	io.ByteWriter

	// ClosePath closes the path by sending an empty frame, after which no more data may be written to it.
	ClosePath() error

	Index[IndexWriteCloser]

	// WriteBytes writes `p` without copying it, `p` must not be modified afterwards
	WriteBytes(p []byte) error
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

// JoinDeferred returns a function, which calls all non-nil `fs` concurrently and returns their joined errors.
// It returns `nil` if all `fs` are `nil`.
func JoinDeferred(fs ...func(context.Context) error) func(context.Context) error {
	fs = slices.DeleteFunc(fs, func(f func(context.Context) error) bool { return f == nil })
	switch len(fs) {
	case 0:
		return nil
	case 1:
		return fs[0]
	}
	return func(ctx context.Context) error {
		var g errGroup
		for _, f := range fs {
			g.Go(func() error { return f(ctx) })
		}
		return g.Wait(nil)
	}
}

// IndexWrite returns a function, which calls `write` with the writer indexed by `path` and closes the indexed writer afterwards.
// It returns `nil` if `write` is `nil`.
func IndexWrite(write func(context.Context, IndexWriter) error, path ...uint32) func(context.Context, IndexWriter) error {
	if write == nil {
		return nil
	}
	return func(ctx context.Context, w IndexWriter) error {
		wi, err := w.Index(path...)
		if err != nil {
			return fmt.Errorf("failed to index nested writer: %w", err)
		}
		return writeNested(ctx, wi, write)
	}
}

// JoinWrites returns a function, which calls all `writes` concurrently with the writer indexed by their key,
// closes the indexed writers afterwards and returns the joined errors.
// It returns `nil` if `writes` is empty.
func JoinWrites(writes map[uint32]func(context.Context, IndexWriter) error) func(context.Context, IndexWriter) error {
	if len(writes) == 0 {
		return nil
	}
	return func(ctx context.Context, w IndexWriter) error {
		var g errGroup
		for index, write := range writes {
			wi, err := w.Index(index)
			if err != nil {
				return g.Wait(fmt.Errorf("failed to index nested writer at index `%d`: %w", index, err))
			}
			g.Go(func() error { return writeNested(ctx, wi, write) })
		}
		return g.Wait(nil)
	}
}

type ByteReadCloser interface {
	ByteReader
	io.Closer
}

type BufReadCloser struct {
	*bufio.Reader
	io.Closer
}
