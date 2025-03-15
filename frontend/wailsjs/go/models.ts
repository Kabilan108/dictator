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
	    apiUrl: string;
	    apiKey: string;
	    defaultModel: string;
	    supportsModels: boolean;
	
	    static createFrom(source: any = {}) {
	        return new WhisperSettings(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.apiUrl = source["apiUrl"];
	        this.apiKey = source["apiKey"];
	        this.defaultModel = source["defaultModel"];
	        this.supportsModels = source["supportsModels"];
	    }
	}

}

