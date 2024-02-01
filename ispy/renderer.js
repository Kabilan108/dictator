// renderer.js

const info = document.getElementById('info');
info.innerText = `This app isinformation using Chrome (v${versions.chrome()}), Node.js (v${versions.node()}), and Electron (v${versions.electron()})`

const func = async () => {
    const response = await window.versions.ping();
    console.log(response);
}

func().catch(console.error); // Call the function and catch any potential errors
