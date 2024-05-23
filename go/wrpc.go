package wrpc

import (
	"context"
	"errors"
	"fmt"
	"io"
)

type Invoker interface {
	Invoke(ctx context.Context, instance string, name string, f func(IndexWriter, IndexReadCloser) error, subs ...SubscribePath) error
}

type Server interface {
	Serve(instance string, name string, f func(context.Context, IndexWriter, IndexReadCloser) error, subs ...SubscribePath) (func() error, error)
}

// Own is an owned resource handle
type Own[T any] string

func (v Own[T]) Drop(ctx context.Context, c Invoker) error {
	if v == "" {
		return errors.New("cannot drop a resource without an ID")
	}
	return c.Invoke(ctx, string(v), "drop", func(w IndexWriter, r IndexReadCloser) error {
		_, err := w.Write(nil)
		if err != nil {
			return fmt.Errorf("failed to write empty `drop` parameters")
		}
		_, err = r.Read(nil)
		if err != nil {
			return fmt.Errorf("failed to read empty `drop` result")
		}
		return nil
	})
}

func (v Own[T]) Borrow() Borrow[T] {
	return Borrow[T](v)
}

// Borrow is a borrowed resource handle
type Borrow[T any] string

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

	Index[IndexReader]
}

type IndexWriter interface {
	io.Writer
	io.ByteWriter

	Index[IndexWriter]
}

type IndexReadCloser interface {
	IndexReader
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

type Completer interface {
	IsComplete() bool
}

type Receiver[T any] interface {
	Receive() (T, error)
}

type ReceiveCompleter[T any] interface {
	Receiver[T]
	Completer
}

type ReadCompleter interface {
	io.Reader
	Completer
}

type ByteReadCompleter interface {
	ByteReader
	Completer
}

type PendingReceiver[T any] struct {
	Receiver[T]
}

func (r *PendingReceiver[T]) Receive() (T, error) {
	return r.Receiver.Receive()
}

func (*PendingReceiver[T]) IsComplete() bool {
	return false
}

func NewPendingReceiver[T any](rx Receiver[T]) *PendingReceiver[T] {
	return &PendingReceiver[T]{rx}
}

type CompleteReceiver[T any] struct {
	v     T
	ready bool
}

func (r *CompleteReceiver[T]) Receive() (T, error) {
	defer func() {
		*r = CompleteReceiver[T]{}
	}()
	if !r.ready {
		return r.v, io.EOF
	}
	return r.v, nil
}

func (*CompleteReceiver[T]) IsComplete() bool {
	return true
}

func NewCompleteReceiver[T any](v T) *CompleteReceiver[T] {
	return &CompleteReceiver[T]{v, true}
}

type DecodeReceiver[T any] struct {
	r      IndexReader
	decode func(IndexReader) (T, error)
}

func (r *DecodeReceiver[T]) Receive() (T, error) {
	return r.decode(r.r)
}

func (*DecodeReceiver[T]) IsComplete() bool {
	return false
}

func (r *DecodeReceiver[T]) Close() error {
	c, ok := r.r.(io.Closer)
	if ok {
		return c.Close()
	}
	return nil
}

func NewDecodeReceiver[T any](r IndexReader, decode func(IndexReader) (T, error)) *DecodeReceiver[T] {
	return &DecodeReceiver[T]{r, decode}
}
