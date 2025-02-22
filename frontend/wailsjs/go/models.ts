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

}

