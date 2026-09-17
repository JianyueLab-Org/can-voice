// can-voice 是 Cerulean Aviation Network 的语音服务端。
//
// 无状态：没有数据库、没有持久化、没有 ACL。进程重启等于所有人重连并重发 SUB。
// 这也意味着没有"丢了某个卷就全网瘫痪且无法远程修复"的风险（spec 8）。
package main

import (
	"context"
	"log/slog"
	"os"
	"os/signal"
	"syscall"

	"github.com/JianyueLab-Org/can-voice/server/internal/fsdfeed"
	"github.com/JianyueLab-Org/can-voice/server/internal/geo"
	"github.com/JianyueLab-Org/can-voice/server/internal/router"
	"github.com/JianyueLab-Org/can-voice/server/internal/tlscert"
	"github.com/JianyueLab-Org/can-voice/server/internal/transport"
)

func main() {
	slog.SetDefault(slog.New(slog.NewJSONHandler(os.Stderr, &slog.HandlerOptions{
		Level: levelFromEnv(),
	})))

	cfg, err := LoadConfig(os.Getenv)
	if err != nil {
		slog.Error("configuration is incomplete", "error", err)
		os.Exit(1)
	}
	// 不是 tls.LoadX509KeyPair 一次：Let's Encrypt 续期只换磁盘上的文件，
	// 读一次的话旧证书过期那天全网连不上，而进程还活着（#68）。
	certs, err := tlscert.Load(cfg.Cert, cfg.Key)
	if err != nil {
		slog.Error("cannot load the TLS key pair", "error", err)
		os.Exit(1)
	}

	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer stop()

	r := router.New()
	// 兜底半径表在**开始服务之前**装好：`geo.UseTable` 是 set-once，
	// 之后每一包的扇出路径都在读它。
	geo.UseTable(cfg.Ranges)

	feed := fsdfeed.NewFeed(cfg.FeedURL)
	r.SetLocator(feed)
	go feed.Run(ctx)

	slog.Info("starting", "addr", cfg.Addr, "feed", cfg.FeedURL, "max_rx", cfg.MaxRX)
	if err := transport.Serve(ctx, transport.Config{
		Addr:      cfg.Addr,
		TLS:       certs.Config(),
		PublicKey: cfg.PubKey,
		MaxRX:     cfg.MaxRX,
	}, r); err != nil && ctx.Err() == nil {
		slog.Error("server stopped", "error", err)
		os.Exit(1)
	}
	slog.Info("shut down cleanly")
}

func levelFromEnv() slog.Level {
	if os.Getenv("CAN_VOICE_DEBUG") != "" {
		return slog.LevelDebug
	}
	return slog.LevelInfo
}
