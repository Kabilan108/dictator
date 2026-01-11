package streaming

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/url"
	"sync"
	"time"

	"github.com/gorilla/websocket"
)

const (
	MsgTypeConfig  = "config"
	MsgTypeAudio   = "audio"
	MsgTypeEnd     = "end"
	MsgTypePartial = "partial"
	MsgTypeFinal   = "final"
	MsgTypeError   = "error"
)

type ClientMessage struct {
	Type        string `json:"type"`
	Data        string `json:"data,omitempty"`
	Seq         int    `json:"seq,omitempty"`
	ChunkFrames int    `json:"chunk_frames,omitempty"`
}

type ServerMessage struct {
	Type      string `json:"type"`
	Text      string `json:"text,omitempty"`
	StableLen int    `json:"stable_len,omitempty"`
	Seq       int    `json:"seq,omitempty"`
	Message   string `json:"message,omitempty"`
	Code      string `json:"code,omitempty"`
}

type PartialHandler func(text string, stableLen int, seq int)
type FinalHandler func(text string)
type ErrorHandler func(err error)

type Client struct {
	endpoint    string
	apiKey      string
	chunkFrames int

	conn   *websocket.Conn
	connMu sync.Mutex
	seq    int

	onPartial PartialHandler
	onFinal   FinalHandler
	onError   ErrorHandler

	ctx    context.Context
	cancel context.CancelFunc
	done   chan struct{}
}

func NewClient(endpoint, apiKey string, chunkFrames int) *Client {
	return &Client{
		endpoint:    endpoint,
		apiKey:      apiKey,
		chunkFrames: chunkFrames,
		done:        make(chan struct{}),
	}
}

func (c *Client) SetHandlers(onPartial PartialHandler, onFinal FinalHandler, onError ErrorHandler) {
	c.onPartial = onPartial
	c.onFinal = onFinal
	c.onError = onError
}

func (c *Client) Connect(ctx context.Context) error {
	c.ctx, c.cancel = context.WithCancel(ctx)

	u, err := url.Parse(c.endpoint)
	if err != nil {
		return fmt.Errorf("invalid endpoint: %w", err)
	}

	q := u.Query()
	q.Set("token", c.apiKey)
	u.RawQuery = q.Encode()

	dialer := websocket.Dialer{
		HandshakeTimeout: 10 * time.Second,
	}

	conn, _, err := dialer.DialContext(c.ctx, u.String(), nil)
	if err != nil {
		return fmt.Errorf("websocket connect failed: %w", err)
	}

	c.connMu.Lock()
	c.conn = conn
	c.connMu.Unlock()

	configMsg := ClientMessage{
		Type:        MsgTypeConfig,
		ChunkFrames: c.chunkFrames,
	}
	if err := c.sendMessage(configMsg); err != nil {
		conn.Close()
		return fmt.Errorf("failed to send config: %w", err)
	}

	go c.receiveLoop()

	return nil
}

func (c *Client) SendAudio(pcmData []byte) error {
	c.seq++
	msg := ClientMessage{
		Type: MsgTypeAudio,
		Data: base64.StdEncoding.EncodeToString(pcmData),
		Seq:  c.seq,
	}
	return c.sendMessage(msg)
}

func (c *Client) End() error {
	msg := ClientMessage{Type: MsgTypeEnd}
	return c.sendMessage(msg)
}

func (c *Client) Close() error {
	if c.cancel != nil {
		c.cancel()
	}

	c.connMu.Lock()
	defer c.connMu.Unlock()

	if c.conn != nil {
		err := c.conn.Close()
		c.conn = nil
		return err
	}
	return nil
}

func (c *Client) Wait() {
	<-c.done
}

func (c *Client) sendMessage(msg ClientMessage) error {
	c.connMu.Lock()
	defer c.connMu.Unlock()

	if c.conn == nil {
		return fmt.Errorf("not connected")
	}

	data, err := json.Marshal(msg)
	if err != nil {
		return err
	}

	return c.conn.WriteMessage(websocket.TextMessage, data)
}

func (c *Client) receiveLoop() {
	defer close(c.done)

	for {
		select {
		case <-c.ctx.Done():
			return
		default:
		}

		c.connMu.Lock()
		conn := c.conn
		c.connMu.Unlock()

		if conn == nil {
			return
		}

		_, data, err := conn.ReadMessage()
		if err != nil {
			if c.ctx.Err() != nil {
				return
			}
			if c.onError != nil {
				c.onError(fmt.Errorf("read error: %w", err))
			}
			return
		}

		var msg ServerMessage
		if err := json.Unmarshal(data, &msg); err != nil {
			if c.onError != nil {
				c.onError(fmt.Errorf("json decode error: %w", err))
			}
			continue
		}

		switch msg.Type {
		case MsgTypePartial:
			if c.onPartial != nil {
				c.onPartial(msg.Text, msg.StableLen, msg.Seq)
			}
		case MsgTypeFinal:
			if c.onFinal != nil {
				c.onFinal(msg.Text)
			}
			return
		case MsgTypeError:
			if c.onError != nil {
				c.onError(fmt.Errorf("server error [%s]: %s", msg.Code, msg.Message))
			}
			return
		}
	}
}
