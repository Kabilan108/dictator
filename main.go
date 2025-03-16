package main

import (
	"embed"

	"github.com/wailsapp/wails/v2"
	"github.com/wailsapp/wails/v2/pkg/logger"
	"github.com/wailsapp/wails/v2/pkg/options"
	"github.com/wailsapp/wails/v2/pkg/options/assetserver"
	"github.com/wailsapp/wails/v2/pkg/options/mac"
	"github.com/wailsapp/wails/v2/pkg/options/windows"

	"dictator/app"
)

//go:embed all:frontend/dist
var assets embed.FS

//go:embed build/appicon.png
var icon []byte

func main() {
	// Create an instance of the dictator structure
	dictator := NewApp()

	// Create application with options
	err := wails.Run(&options.App{
		Title:             "dictator",
		Width:             375,
		MinWidth:          375,
		MaxWidth:          375,
		Height:            180, // Match DEFAULT_WINDOW_HEIGHT from frontend
		MinHeight:         150,
		MaxHeight:         600,
		DisableResize:     false,
		Fullscreen:        false,
		Frameless:         false,
		StartHidden:       false,
		HideWindowOnClose: false,
		BackgroundColour:  &options.RGBA{R: 30, G: 32, B: 33, A: 255},
		AssetServer: &assetserver.Options{
			Assets: assets,
		},
		Menu:             nil,
		Logger:           nil,
		LogLevel:         logger.DEBUG,
		OnStartup:        dictator.startup,
		OnDomReady:       dictator.domReady,
		OnBeforeClose:    dictator.beforeClose,
		OnShutdown:       dictator.shutdown,
		WindowStartState: options.Normal,
		Bind: []interface{}{
			dictator,
		},
		// Windows platform specific options
		Windows: &windows.Options{
			WebviewIsTransparent: false,
			WindowIsTranslucent:  false,
			DisableWindowIcon:    false,
			// DisableFramelessWindowDecorations: false,
			WebviewUserDataPath: "",
			ZoomFactor:          1.0,
		},
		// Mac platform specific options
		Mac: &mac.Options{
			TitleBar: &mac.TitleBar{
				TitlebarAppearsTransparent: true,
				HideTitle:                  false,
				HideTitleBar:               false,
				FullSizeContent:            false,
				UseToolbar:                 false,
				HideToolbarSeparator:       true,
			},
			Appearance:           mac.NSAppearanceNameDarkAqua,
			WebviewIsTransparent: true,
			WindowIsTranslucent:  true,
			About: &mac.AboutInfo{
				Title:   "dictator",
				Message: "",
				Icon:    icon,
			},
		},
	})
	if err != nil {
		app.Log.E("Failed to start wails app: %v", err)
	}
}
