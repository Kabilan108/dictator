export namespace app {
	
	export class ModelInfo {
	    id: string;
	
	    static createFrom(source: any = {}) {
	        return new ModelInfo(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.id = source["id"];
	    }
	}

}

export namespace main {
	
	export class Result {
	    success: boolean;
	    transcript?: string;
	    error?: string;
	
	    static createFrom(source: any = {}) {
	        return new Result(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.success = source["success"];
	        this.transcript = source["transcript"];
	        this.error = source["error"];
	    }
	}
	export class WhisperSettings {
	    api_url: string;
	    api_key: string;
	    default_model: string;
	    supports_models: boolean;
	
	    static createFrom(source: any = {}) {
	        return new WhisperSettings(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.api_url = source["api_url"];
	        this.api_key = source["api_key"];
	        this.default_model = source["default_model"];
	        this.supports_models = source["supports_models"];
	    }
	}

}

